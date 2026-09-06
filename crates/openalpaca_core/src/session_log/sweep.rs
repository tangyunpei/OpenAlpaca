//! The boot pass that enforces §5.4's **global** session-log cap.
//!
//! Two caps bound the log. The per-session one (`log_max_session_bytes`) lives
//! in the writer, because §5.4 puts it there — "enforcement runs in the writer
//! task on rotation (cheap, no scan)", plus after a spill, which is the other
//! way `results/` grows. The cross-session one is this:
//!
//! > `log_max_total_bytes` — 2 GB — Across all sessions. Evict oldest-touched
//! > **archived** sessions' logs first, LRU; an active session's log is never
//! > evicted.
//!
//! and §5.4 says where it runs: "once at boot for the global cap".
//!
//! **What it removes, and in what order.** Within one archived session,
//! least-destructive first: the `results/` spill files, then the rotated
//! `log.<first>-<last>.jsonl` segments. It never removes a **live** segment —
//! the per-session trim is explicit that whole oldest segments go "never the
//! live segment", and a global pass has no better claim on one. So a swept
//! session keeps its narrative and loses the payloads and the head, which is
//! exactly the trade §5.3's source-of-truth split makes safe: chat content is
//! in SQLite, and replay resume only ever reads the tail.
//!
//! **What it never touches**: an active session, anything under `snapshots/`
//! (reserved for Phase 8), and any name at the sessions root that this store
//! did not create — §1.3 rule 3. Unknown names are *counted* (they are taking
//! the disk the cap is about) but never removed.
//!
//! **Its only record is the deletions themselves**, so a crash between two of
//! them leaves a partially swept root and the next boot simply continues: the
//! pass is idempotent, and a file already gone is not an error.

use super::reader::{LIVE_SEGMENT, segment_range};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// What one pass did — the one-line summary the daemon logs at boot.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    /// Session directories examined.
    pub sessions_visited: usize,
    /// Sessions the pass removed something from.
    pub sessions_evicted: usize,
    pub files_removed: usize,
    pub bytes_freed: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
    /// True when the cap could not be met because everything still over it is
    /// protected — an active session, or a live segment. Reported rather than
    /// hidden: the alternative is deleting something §5.4 says must stay.
    pub over_cap_after: bool,
}

/// Bring `root` under `max_total_bytes`, evicting oldest-touched archived
/// sessions first.
///
/// `active` is the set of session ids that must not be touched — the sessions
/// the database still calls active. Ids are matched against the **directory
/// name**, which is `session_dir_name(id)`, so the caller passes raw ids.
pub fn enforce_total_cap(
    root: &Path,
    max_total_bytes: u64,
    active: &HashSet<String>,
) -> io::Result<SweepReport> {
    let (mut sessions, foreign_bytes) = match scan(root, active) {
        Ok(scanned) => scanned,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(SweepReport::default()),
        Err(e) => return Err(e),
    };

    let bytes_before: u64 = sessions.iter().map(|s| s.bytes).sum::<u64>() + foreign_bytes;
    let mut report = SweepReport {
        sessions_visited: sessions.len(),
        bytes_before,
        bytes_after: bytes_before,
        ..Default::default()
    };
    if bytes_before <= max_total_bytes {
        return Ok(report);
    }

    // LRU: the least recently touched archived session gives up its bytes
    // first. An active one is never a candidate, whatever its age.
    sessions.sort_by_key(|s| s.touched);
    let mut total = bytes_before;
    for session in sessions {
        if total <= max_total_bytes {
            break;
        }
        if session.protected {
            continue;
        }
        let mut freed_here = 0;
        for victim in session.evictable {
            if total <= max_total_bytes {
                break;
            }
            // A file already gone is a previous pass that did not finish, not
            // a failure: the sweep's only record is the deletion itself.
            match fs::remove_file(&victim.path) {
                Ok(()) => {
                    total = total.saturating_sub(victim.bytes);
                    freed_here += victim.bytes;
                    report.files_removed += 1;
                    report.bytes_freed += victim.bytes;
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => {
                    tracing::warn!(path = %victim.path.display(), "Session log sweep: {e}");
                }
            }
        }
        if freed_here > 0 {
            report.sessions_evicted += 1;
        }
    }

    report.bytes_after = total;
    report.over_cap_after = total > max_total_bytes;
    Ok(report)
}

/// One candidate file, in the order it may be given up.
struct Victim {
    path: PathBuf,
    bytes: u64,
}

struct SessionDir {
    /// Newest mtime among the session's own files: "oldest-touched" in §5.4's
    /// LRU. Taken from the files rather than the directory because a directory
    /// mtime changes for reasons that have nothing to do with the session.
    touched: u64,
    bytes: u64,
    /// An active session — counted towards the total, never evicted from.
    protected: bool,
    /// Least destructive first: `results/` spills, then rotated segments. The
    /// live segment is never a candidate.
    evictable: Vec<Victim>,
}

/// The session directories, plus the bytes held by names at the sessions root
/// that this store did not create.
fn scan(root: &Path, active: &HashSet<String>) -> io::Result<(Vec<SessionDir>, u64)> {
    let protected: HashSet<String> = active
        .iter()
        .map(|id| super::session_dir_name(id))
        .collect();
    let mut out = Vec::new();
    let mut foreign = 0;
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if !path.is_dir() {
            // A stray file at the sessions root is not this store's to delete
            // (§1.3 rule 3) — but it is taking the disk the cap is about, so
            // it is counted and never touched.
            foreign += entry.metadata().map(|m| m.len()).unwrap_or(0);
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        out.push(scan_session(&path, protected.contains(&name)));
    }
    Ok((out, foreign))
}

fn scan_session(dir: &Path, protected: bool) -> SessionDir {
    let mut bytes = 0;
    let mut touched = 0;
    let mut spills: Vec<Victim> = Vec::new();
    let mut segments: Vec<(u64, Victim)> = Vec::new();

    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            // `snapshots/` is reserved for Phase 8 and is not the sweep's to
            // empty; its bytes still count.
            let (dir_bytes, dir_touched, files) = walk(&path);
            bytes += dir_bytes;
            touched = touched.max(dir_touched);
            if name == super::RESULTS_DIR {
                spills.extend(files);
            }
            continue;
        }
        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
        bytes += len;
        touched = touched.max(mtime_secs(&path));
        // §5.4 drops "whole oldest segments … never the live segment", and a
        // global pass has no better claim on a live one than the writer does.
        if name != LIVE_SEGMENT && segment_range(&name).is_some() {
            let first = segment_range(&name).map(|(first, _)| first).unwrap_or(0);
            segments.push((first, Victim { path, bytes: len }));
        }
    }

    // Oldest first inside each class, and the payloads before the narrative.
    spills.sort_by(|a, b| mtime_secs(&a.path).cmp(&mtime_secs(&b.path)));
    segments.sort_by_key(|(first, _)| *first);
    let mut evictable = spills;
    evictable.extend(segments.into_iter().map(|(_, victim)| victim));

    SessionDir {
        touched,
        bytes,
        protected,
        evictable,
    }
}

/// Bytes, newest mtime and the files under `dir`, recursively.
fn walk(dir: &Path) -> (u64, u64, Vec<Victim>) {
    let mut bytes = 0;
    let mut touched = 0;
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            let (sub_bytes, sub_touched, _) = walk(&path);
            bytes += sub_bytes;
            touched = touched.max(sub_touched);
            continue;
        }
        let len = entry.metadata().map(|m| m.len()).unwrap_or(0);
        bytes += len;
        touched = touched.max(mtime_secs(&path));
        files.push(Victim { path, bytes: len });
    }
    (bytes, touched, files)
}

fn mtime_secs(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        })
}

//! Reading a crashed run's undelivered interjections back out of its log
//! (§5.6b).
//!
//! > The sweep additionally **reads each open session's JSONL tail**:
//! > `steering` records with no subsequent `steering_drained` containing their
//! > request_id are converted to
//! > `lane_followups(kind='unprocessed_steering', session_id=…)` — the exact
//! > rows the graceful path already writes — so a crash no longer silently
//! > eats interjections.
//!
//! The graceful path is `dispatcher/lead_agent.rs`: when a workflow detaches,
//! it drains whatever is left in the [`SteeringInbox`] and files each message
//! as a follow-up row. A `kill -9` never runs that code, and the inbox is
//! process memory, so the log is the only place those messages still exist.
//!
//! This module answers one question — *which of a run's interjections did the
//! loop never see?* — and answers it from the two records that know:
//! `steering` (written on every accepted push, `runner/steering.rs`) and
//! `steering_drained` (written at both drain sites,
//! `runner/agentic_loop/mod.rs`, naming the request ids it took). A push with
//! no matching drain is an interjection the model was never shown.
//!
//! **Why the whole log and not "after the last round boundary".** A drain is
//! recorded by request id, so membership is the exact test and position is
//! not needed: `seq` is monotonic and a drain can only follow its own push, so
//! scanning for drained ids anywhere in the log gives the same answer as
//! scanning after the last boundary, without having to decide what the last
//! boundary was in a log whose tail may be torn.
//!
//! **Memory.** The log is paged, never read whole: a session may hold up to
//! `log_max_session_bytes` (256 MB) and this runs at boot. Only the run's own
//! `steering` records and the drained id set are kept.

use super::reader::{LoggedRecord, read_records_after};
use std::collections::HashSet;
use std::io;
use std::path::Path;

/// How many records one page of the scan holds. Bounds the pass's memory to a
/// page plus the run's own interjections, whatever the log's size.
const PAGE: usize = 512;

/// One interjection the loop never delivered — everything
/// `FollowupRepository::queue` needs to file it as the graceful path would.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndrainedSteering {
    /// The push's `request_id`. The identity §5.6b's idempotence is about;
    /// carried so a caller can log exactly which interjection it recovered.
    pub request_id: String,
    /// The user's words, verbatim — the follow-up row's `content`.
    pub text: String,
    /// The serialised [`Principal`](crate::security::policy::Principal), as
    /// `principal_json`. The column is NOT NULL, so a record that predates
    /// this field is skipped rather than filed under an invented identity.
    pub principal_json: String,
    /// The turn's project root, or `None` when it had none.
    pub workspace_path: Option<String>,
}

/// What one scan found, so a caller can say what it could *not* recover
/// instead of silently dropping it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SteeringScan {
    pub undrained: Vec<UndrainedSteering>,
    /// Records that were undrained but carried no `principal` — written by a
    /// build before the field existed. Counted, never invented.
    pub unrecoverable: usize,
}

/// The interjections `task_id` was pushed and never shown, oldest first.
///
/// `session_dir` is the run's session directory
/// (`sessions/<session_dir_name(id)>`). A directory that does not exist is not
/// an error — a session whose writer never ran has nothing to recover — and
/// answers an empty scan.
pub fn undrained_steering(session_dir: &Path, task_id: &str) -> io::Result<SteeringScan> {
    if !session_dir.is_dir() {
        return Ok(SteeringScan::default());
    }

    let mut drained: HashSet<String> = HashSet::new();
    let mut pushed: Vec<(String, LoggedRecord)> = Vec::new();
    let mut cursor: Option<u64> = None;

    loop {
        let page = read_records_after(session_dir, cursor, PAGE)?;
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|r| r.seq);
        let full = page.len() == PAGE;
        for record in page {
            // A session holds every run started from that conversation, so the
            // task id is what separates this crash from the run beside it.
            if record.task_id.as_deref() != Some(task_id) {
                continue;
            }
            match record.kind.as_str() {
                "steering_drained" => {
                    if let Some(ids) = record.data.get("request_ids").and_then(|v| v.as_array()) {
                        drained.extend(ids.iter().filter_map(|v| v.as_str().map(str::to_string)));
                    }
                }
                "steering" => {
                    if let Some(id) = record.data.get("request_id").and_then(|v| v.as_str()) {
                        pushed.push((id.to_string(), record));
                    }
                }
                _ => {}
            }
        }
        if !full {
            break;
        }
    }

    let mut scan = SteeringScan::default();
    for (request_id, record) in pushed {
        if drained.contains(&request_id) {
            continue;
        }
        let Some(text) = record.data.get("text").and_then(|v| v.as_str()) else {
            scan.unrecoverable += 1;
            continue;
        };
        // The column is NOT NULL and a `Principal` cannot be guessed from a
        // lane key: an older record is reported as unrecoverable rather than
        // filed under an identity nobody asserted.
        let principal = record.data.get("principal");
        let Some(principal_json) = principal
            .filter(|v| !v.is_null())
            .map(|v| v.to_string())
        else {
            scan.unrecoverable += 1;
            continue;
        };
        scan.undrained.push(UndrainedSteering {
            request_id,
            text: text.to_string(),
            principal_json,
            workspace_path: record
                .data
                .get("workspace_path")
                .and_then(|v| v.as_str())
                .map(str::to_string),
        });
    }
    Ok(scan)
}

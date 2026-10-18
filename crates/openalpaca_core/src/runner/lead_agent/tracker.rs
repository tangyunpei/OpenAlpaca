use crate::runner::MAX_TOOL_RESULT_SIZE;
use crate::utils::text::head_tail;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// Status of a background-spawned subagent.
#[derive(Debug, Clone)]
pub enum SubagentStatus {
    Queued,
    Running,
    Completed {
        content: String,
        success: bool,
    },
    /// `error` says why; `output` is the subagent's whole final content when
    /// its loop produced one (empty for a plugin loop's failure and for a run
    /// cancelled before it started). Kept whole, as a completed answer is:
    /// the lead reads it with `check_subagent_status`.
    Failed {
        error: String,
        output: String,
    },
}

/// What `wait_for_subagents` adds around the answers: its header (the
/// interrupted return is the longest, about 180 bytes) and the line naming
/// the run ids reported earlier ([`REPORTED_IDS_LIST_BYTES`] of ids and
/// about 150 bytes of words, N-LEAD-3).
const SUMMARY_HEADER_RESERVE_BYTES: usize = 1024;
/// How many bytes of run ids the earlier-reported line names before it
/// counts the rest, so the line stays inside the header reserve.
const REPORTED_IDS_LIST_BYTES: usize = 512;
/// One entry's fixed cost: `- **<run_id>**: completed — `, up to two
/// head/tail notices, ` Last output: `, the join, and a failed entry's error
/// cut to 512 bytes.
const SUMMARY_ENTRY_OVERHEAD_BYTES: usize = 768;
/// One answer never takes more of the lead's context than this, however few
/// subagents ran — `check_subagent_status` returns the whole answer.
const SUMMARY_ANSWER_CAP_BYTES: usize = 8 * 1024;
/// Below this an entry says nothing useful; from about 25 entries on the
/// summary may exceed the bound and the loop's own spill or cut takes over.
const SUMMARY_ANSWER_FLOOR_BYTES: usize = 512;
/// A failed entry's error text, which `LoopFinishReason::Error(String)` and a
/// plugin's error make unbounded.
const SUMMARY_ERROR_MAX_BYTES: usize = 512;

/// How many bytes of each answer a summary shows: the loop's inline bound,
/// less the header, shared by every entry the summary lists, less each
/// entry's fixed cost — floored and capped. For the default 32 KiB bound and
/// a batch of 8: 3,200 bytes an answer, 1,600 head and 1,600 tail, so the
/// worst case `8 × (3,200 + 768) + 1,024` is exactly the bound. Stage 0's F2
/// shared it among every entry the run had spawned; since N-LEAD-3
/// `wait_for_subagents` lists only the entries it has not reported yet, so a
/// second batch of 8 gets the same 3,200 bytes an answer as the first.
pub(super) fn answer_budget(inline_bytes: usize, entries: usize) -> usize {
    let per_entry = inline_bytes.saturating_sub(SUMMARY_HEADER_RESERVE_BYTES) / entries.max(1);
    per_entry
        .saturating_sub(SUMMARY_ENTRY_OVERHEAD_BYTES)
        .clamp(SUMMARY_ANSWER_FLOOR_BYTES, SUMMARY_ANSWER_CAP_BYTES)
}

/// Shared tracker for background subagent tasks.
/// Allows the lead agent to spawn multiple subagents concurrently
/// and check/wait for their results.
pub struct SubagentTracker {
    pub statuses: Mutex<HashMap<String, SubagentStatus>>,
    /// The finished entries whose answer has reached the lead — through
    /// `wait_for_subagents`'s head and tail, or `check_subagent_status`'s
    /// whole answer — which a later wait names instead of repeating
    /// (N-LEAD-3). Locked after `statuses` when both are held.
    reported: Mutex<HashSet<String>>,
    /// Notifies waiters when a subagent completes or fails.
    pub notify: tokio::sync::Notify,
    /// `[orchestrator.sessions] tool_result_inline_bytes` of the loop that
    /// reads `summary()` — the bound the whole summary is sized to stay under.
    inline_bytes: usize,
    /// Whether the lead's run keeps a conversation log, so a tool result
    /// past `inline_bytes` reaches the lead as a stub it pages whole with
    /// `read_result`; with none, it is cut after its first part.
    run_has_log: bool,
    /// Test-only: run once, right after an `all_done()` that answered
    /// `false` has released the lock — a status change landing just after
    /// the waiter's check, which no await point lets a test reach otherwise.
    #[cfg(test)]
    after_unfinished_check: Mutex<Option<AfterCheckHook>>,
}

#[cfg(test)]
type AfterCheckHook = Box<dyn FnOnce(&SubagentTracker) + Send>;

impl Default for SubagentTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentTracker {
    /// A tracker sized to the loop's default inline bound (32 KiB) — tests
    /// and any caller with no `LoopConfig` of its own.
    pub fn new() -> Self {
        Self::with_inline_bytes(MAX_TOOL_RESULT_SIZE)
    }

    /// A tracker whose `summary()` stays under `inline_bytes` for a batch of
    /// eight: the lead passes its own `tool_result_inline_bytes`.
    pub fn with_inline_bytes(inline_bytes: usize) -> Self {
        Self {
            statuses: Mutex::new(HashMap::new()),
            reported: Mutex::new(HashSet::new()),
            notify: tokio::sync::Notify::new(),
            inline_bytes,
            run_has_log: false,
            #[cfg(test)]
            after_unfinished_check: Mutex::new(None),
        }
    }

    /// The same tracker, for a run that keeps a conversation log (`true`)
    /// or none: whether a long `check_subagent_status` answer reaches the
    /// lead whole (N-LEAD-3).
    pub fn with_log(mut self, run_has_log: bool) -> Self {
        self.run_has_log = run_has_log;
        self
    }

    /// Test-only: `hook` runs once, after the next `all_done()` that answers
    /// `false`, with the lock released and before that answer is returned.
    #[cfg(test)]
    pub(super) fn after_next_unfinished_check(
        &self,
        hook: impl FnOnce(&SubagentTracker) + Send + 'static,
    ) {
        *self
            .after_unfinished_check
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(Box::new(hook));
    }

    pub fn register(&self, run_id: &str) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), SubagentStatus::Queued);
    }

    pub fn complete(&self, run_id: &str, content: String, success: bool) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(
            run_id.to_string(),
            SubagentStatus::Completed { content, success },
        );
        drop(map);
        self.notify.notify_waiters();
    }

    /// A failure that carries no answer — cancelled before it started, or a
    /// plugin loop's error.
    pub fn fail(&self, run_id: &str, error: String) {
        self.fail_with_output(run_id, error, String::new());
    }

    /// A failure with the subagent's whole final content beside it.
    pub fn fail_with_output(&self, run_id: &str, error: String, output: String) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), SubagentStatus::Failed { error, output });
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn set_status(&self, run_id: &str, status: SubagentStatus) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), status);
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn get(&self, run_id: &str) -> Option<SubagentStatus> {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.get(run_id).cloned()
    }

    pub fn all_done(&self) -> bool {
        let done = {
            let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
            map.values().all(|s| {
                matches!(
                    s,
                    SubagentStatus::Completed { .. } | SubagentStatus::Failed { .. }
                )
            })
        };
        #[cfg(test)]
        if !done {
            let hook = self
                .after_unfinished_check
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .take();
            if let Some(hook) = hook {
                hook(self);
            }
        }
        done
    }

    /// The run ids still queued or running, sorted — what a lead's run end
    /// has left to stop (N-LEAD-1).
    pub fn unfinished(&self) -> Vec<String> {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        let mut ids: Vec<String> = map
            .iter()
            .filter(|(_, s)| matches!(s, SubagentStatus::Queued | SubagentStatus::Running))
            .map(|(id, _)| id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Record `error` for `run_id` unless it already finished — a lane
    /// closed by its guard after its task was aborted (N-LEAD-1).
    pub fn fail_unless_finished(&self, run_id: &str, error: String) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        if map.get(run_id).is_some_and(SubagentStatus::is_finished) {
            return;
        }
        map.insert(
            run_id.to_string(),
            SubagentStatus::Failed {
                error,
                output: String::new(),
            },
        );
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn status_counts(&self) -> (usize, usize, usize, usize) {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        let (mut queued, mut running, mut completed, mut failed) = (0, 0, 0, 0);
        for s in map.values() {
            match s {
                SubagentStatus::Queued => queued += 1,
                SubagentStatus::Running => running += 1,
                SubagentStatus::Completed { .. } => completed += 1,
                SubagentStatus::Failed { .. } => failed += 1,
            }
        }
        (queued, running, completed, failed)
    }

    /// Whether a `check_subagent_status` answer of `len` bytes reaches the
    /// lead whole: it fits under the inline bound, or the run's log spills it
    /// to a stub the lead pages with `read_result`. Past the bound with no
    /// log the loop keeps only its first part (N-LEAD-3).
    pub fn reaches_lead_whole(&self, len: usize) -> bool {
        self.run_has_log || len <= self.inline_bytes
    }

    /// Record that `run_id`'s whole answer reached the lead
    /// (`check_subagent_status`), when it has finished — so a later wait
    /// names it instead of repeating it (N-LEAD-3).
    pub fn mark_reported(&self, run_id: &str) {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        if map.get(run_id).is_some_and(SubagentStatus::is_finished) {
            self.reported
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .insert(run_id.to_string());
        }
    }

    /// Every entry the run has spawned, reported or not, each answer sized
    /// for that many entries — a full view that marks nothing.
    pub fn summary(&self) -> String {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        if map.is_empty() {
            return "No subagents have been spawned yet.".to_string();
        }
        let budget = answer_budget(self.inline_bytes, map.len());
        map.iter()
            .map(|(id, status)| entry_line(id, status, budget))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// What `wait_for_subagents` returns (N-LEAD-3): each entry not reported
    /// yet — a finished one's answer as its head and tail, the byte budget
    /// shared among those entries alone — and one line naming the run ids
    /// reported earlier, which a compacted lead can re-read whole with
    /// `check_subagent_status`. The finished entries it shows are reported
    /// from then on; queued and running ones are listed again next time.
    pub fn report(&self) -> String {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        if map.is_empty() {
            return "No subagents have been spawned yet.".to_string();
        }
        let mut reported = self.reported.lock().unwrap_or_else(|p| p.into_inner());
        let mut fresh: Vec<(&String, &SubagentStatus)> = map
            .iter()
            .filter(|(id, _)| !reported.contains(*id))
            .collect();
        fresh.sort_by(|a, b| a.0.cmp(b.0));
        let mut earlier: Vec<&String> = map.keys().filter(|id| reported.contains(*id)).collect();
        earlier.sort();

        let budget = answer_budget(self.inline_bytes, fresh.len());
        let mut parts: Vec<String> = fresh
            .iter()
            .map(|(id, status)| entry_line(id, status, budget))
            .collect();
        if parts.is_empty() {
            parts.push("No new results since the last report.".to_string());
        }
        if !earlier.is_empty() {
            parts.push(format!(
                "Reported earlier, not repeated here: {}. Read any of them again in full \
                 with check_subagent_status.",
                bounded_ids(&earlier)
            ));
        }
        for (id, status) in fresh {
            if status.is_finished() {
                reported.insert(id.clone());
            }
        }
        parts.join("\n")
    }
}

impl SubagentStatus {
    /// Completed or failed: it has an answer, or an error, to report.
    fn is_finished(&self) -> bool {
        matches!(
            self,
            SubagentStatus::Completed { .. } | SubagentStatus::Failed { .. }
        )
    }
}

/// One entry of a summary: a queued or running entry's state, a finished
/// one's answer as its head and tail within `budget`.
fn entry_line(id: &str, status: &SubagentStatus, budget: usize) -> String {
    match status {
        SubagentStatus::Queued => format!("- **{}**: queued (waiting for execution slot)", id),
        SubagentStatus::Running => format!("- **{}**: still running", id),
        SubagentStatus::Completed { content, success } => format!(
            "- **{}**: {} — {}",
            id,
            if *success {
                "completed"
            } else {
                "completed (partial)"
            },
            head_tail(content, budget)
        ),
        SubagentStatus::Failed { error, output } => {
            let mut line = format!(
                "- **{}**: failed — {}",
                id,
                head_tail(error, SUMMARY_ERROR_MAX_BYTES)
            );
            if !output.is_empty() {
                line.push_str(" Last output: ");
                line.push_str(&head_tail(output, budget));
            }
            line
        }
    }
}

/// `ids` joined by `, `, as many as fit in [`REPORTED_IDS_LIST_BYTES`], then
/// "and N more" for the rest — never empty when `ids` is not.
fn bounded_ids(ids: &[&String]) -> String {
    let mut shown: Vec<&str> = Vec::new();
    let mut used = 0;
    for id in ids {
        let cost = id.len() + if shown.is_empty() { 0 } else { 2 };
        if !shown.is_empty() && used + cost > REPORTED_IDS_LIST_BYTES {
            break;
        }
        used += cost;
        shown.push(id);
    }
    let more = ids.len() - shown.len();
    let shown = shown.join(", ");
    if more == 0 {
        shown
    } else {
        format!("{shown}, and {more} more")
    }
}

use crate::runner::MAX_TOOL_RESULT_SIZE;
use crate::utils::text::head_tail;
use std::collections::HashMap;
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

/// What `wait_for_subagents` prepends to the summary (the interrupted return
/// is the longest, about 180 bytes).
const SUMMARY_HEADER_RESERVE_BYTES: usize = 1024;
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

/// How many bytes of each answer `summary` shows: the loop's inline bound,
/// less the header, shared by every tracked entry, less each entry's fixed
/// cost — floored and capped. For the default 32 KiB bound and a batch of
/// 8: 3,200 bytes an answer, 1,600 head and 1,600 tail, so the worst case
/// `8 × (3,200 + 768) + 1,024` is exactly the bound.
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
    /// Notifies waiters when a subagent completes or fails.
    pub notify: tokio::sync::Notify,
    /// `[orchestrator.sessions] tool_result_inline_bytes` of the loop that
    /// reads `summary()` — the bound the whole summary is sized to stay under.
    inline_bytes: usize,
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
            notify: tokio::sync::Notify::new(),
            inline_bytes,
            #[cfg(test)]
            after_unfinished_check: Mutex::new(None),
        }
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
        if matches!(
            map.get(run_id),
            Some(SubagentStatus::Completed { .. } | SubagentStatus::Failed { .. })
        ) {
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

    pub fn summary(&self) -> String {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        if map.is_empty() {
            return "No subagents have been spawned yet.".to_string();
        }
        let budget = answer_budget(self.inline_bytes, map.len());
        let mut parts = Vec::new();
        for (id, status) in map.iter() {
            match status {
                SubagentStatus::Queued => {
                    parts.push(format!("- **{}**: queued (waiting for execution slot)", id));
                }
                SubagentStatus::Running => {
                    parts.push(format!("- **{}**: still running", id));
                }
                SubagentStatus::Completed { content, success } => {
                    parts.push(format!(
                        "- **{}**: {} — {}",
                        id,
                        if *success {
                            "completed"
                        } else {
                            "completed (partial)"
                        },
                        head_tail(content, budget)
                    ));
                }
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
                    parts.push(line);
                }
            }
        }
        parts.join("\n")
    }
}

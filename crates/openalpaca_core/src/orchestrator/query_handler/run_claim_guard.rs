//! H3 — a chat turn never claims a workflow it did not start.
//!
//! The observed failure: on a lane whose history already held three real
//! delegations, a local model answered *"Started a background workflow called
//! "Guanaco fiber notes" (task id: `9f4c2b71`) …"* in one round, calling no
//! tool. No such task existed; `done` carried no `delegation`; the Work pane
//! showed nothing. The model had learned the *form* of its own earlier replies.
//!
//! H1 (history provenance) and H2 (the relay rules) make that less likely.
//! This is the part that does not depend on the model reading anything: the
//! [`AnswerGuard`] the main loop hands its agentic loop, which reads the
//! finished answer, and — narrowly — refuses one that claims to have started a
//! run no run answers to.
//!
//! **Narrow on purpose, and narrowed again in J1/J2.** A false positive is
//! worse than the lie it guards against: on a second offence the runtime
//! itself replaces a *truthful* answer with "I did not start a workflow". So
//! three things must all hold before an answer is even looked up:
//!
//! 1. the turn's own `start_workflow` cell is empty (it really did not
//!    delegate);
//! 2. the answer **asserts a start** — a start verb near a stated id or near
//!    the word workflow/run/job. A status relay ("Task 9f4c… finished", "your
//!    workflow 372e… is still running") claims no start and is never reviewed;
//! 3. the token it states **looks like a run id** — a UUID, or an 8+ hex run
//!    that is not a plain number. A date, a counter and a bare number are not
//!    ids however they are introduced.
//!
//! And the existence check is the owner's, not the lane's (J1): `task_status`
//! answers about every run this owner started, so relaying one lane's run into
//! another is true and must be left alone. A turn that stated no id pays one
//! string scan and touches no database.

use std::sync::Arc;

use openalpaca_storage::{Database, repository::TaskRepository};

use crate::runner::{AnswerGuard, Correction};
use crate::tools::builtins::StartWorkflowTool;

/// What the model is told when it claims a run it did not start.
const CORRECTIVE_NOTE: &str = "You said a workflow was started, but no start_workflow call was \
                               made in this turn. Either call start_workflow now, or answer \
                               without claiming a run was started.";

/// What the user reads when the model claims it again anyway. The runtime
/// speaking in its own voice, in the pattern of V3's no-answer line.
const REPLACEMENT_LINE: &str = "I did not start a workflow — no run exists for that. Ask again \
                                and I will start one.";

/// The main loop's answer guard.
///
/// Holds the turn's own `start_workflow` instance — the same result cell the
/// handler reads for structured delegation metadata — so "did this turn start
/// a run?" is answered by the tool that would have started it, not by parsing
/// text. `db` is optional: a daemon without one cannot tell a quoted run from
/// an invented one, so it declines to judge rather than guess.
pub(super) struct RunClaimGuard {
    start_workflow: Arc<StartWorkflowTool>,
    db: Option<Database>,
    /// Only for the log line — the claim is judged against `created_by`.
    lane_key: String,
    /// Whose runs count as real here: the same identity `start_workflow`
    /// stamps on a task and `task_status` reads back (`ToolContext::created_by`).
    created_by: String,
}

impl RunClaimGuard {
    pub(super) fn new(
        start_workflow: Arc<StartWorkflowTool>,
        db: Option<Database>,
        lane_key: &str,
        created_by: &str,
    ) -> Self {
        Self {
            start_workflow,
            db,
            lane_key: lane_key.to_string(),
            created_by: created_by.to_string(),
        }
    }
}

impl AnswerGuard for RunClaimGuard {
    fn review(&self, answer: &str) -> Option<Correction> {
        // A turn that really delegated may say so, with the id the call
        // returned. The cell is written only by a successful dispatch.
        if self.start_workflow.outcome().is_some() {
            return None;
        }
        // Cheapest possible on the ordinary turn: nothing claimed, nothing to
        // do, no query.
        let claimed = claimed_run_ids(answer);
        if claimed.is_empty() {
            return None;
        }
        let db = self.db.as_ref()?;
        let repo = TaskRepository::new(db);
        for id in &claimed {
            match repo.owner_has_task_id_prefix(&self.created_by, id) {
                // Quoting a run that exists is ordinary conversation.
                Ok(true) => continue,
                Ok(false) => {
                    tracing::warn!(
                        lane_key = %self.lane_key,
                        claimed_id = %id,
                        "Main-loop answer claims a run id that matches no task of this owner"
                    );
                    return Some(Correction {
                        note: CORRECTIVE_NOTE.to_string(),
                        replacement: REPLACEMENT_LINE.to_string(),
                    });
                }
                // A failed lookup must not invent a lie: say nothing.
                Err(e) => {
                    tracing::warn!(
                        lane_key = %self.lane_key,
                        "Run-claim guard could not check a stated task id: {e}"
                    );
                    return None;
                }
            }
        }
        None
    }
}

/// The cues after which the next token is being presented as a run id.
///
/// Matched case-insensitively against the answer. Deliberately short: the
/// guard's job is the sentence the model learned to imitate, whose shape is
/// always "… (task id: X)" or "… run id X".
const ID_CUES: [&str; 7] = [
    "task id",
    "task_id",
    "task-id",
    "taskid",
    "run id",
    "run_id",
    "task",
];

/// Characters that may sit between a cue and the id it introduces — the
/// punctuation a model wraps an id in. Anything else ends the scan, so
/// "task list" yields nothing.
const ID_LEAD_IN: [char; 10] = [':', '=', '`', '*', '"', '\'', '(', '[', '#', ' '];

/// The words with which an answer asserts that a run was started *in this
/// turn* (J2). Short, lower-case, and phrases rather than bare verbs where a
/// bare verb would swallow a status relay: "is still running" must not read as
/// a start, so only "now running in the background" does.
const START_CUES: [&str; 9] = [
    "started",
    "kicked off",
    "launched",
    "spun up",
    "now running in the background",
    "i've queued",
    "i have queued",
    "i've delegated",
    "i have delegated",
];

/// The things a start verb may be asserted *of*. Matched as whole words.
const START_SUBJECTS: [&str; 3] = ["workflow", "run", "job"];

/// How far apart a start verb and the thing it is asserted of may sit and
/// still belong to the same claim — roughly a long sentence.
const NEAR_WINDOW_BYTES: usize = 120;

/// Ids an answer presents as a task/run id **while claiming to have started
/// one**, lowercased and deduplicated.
///
/// Two conditions, both required (J2):
///
/// - *assertion* — the answer says a run was started: a [`START_CUES`] phrase
///   within [`NEAR_WINDOW_BYTES`] of the stated id, or of a [`START_SUBJECTS`]
///   word. Without one the answer is relaying status, not claiming a start,
///   and nothing in it is a claim.
/// - *shape* — the token sits in an id position (after an [`ID_CUES`] cue and
///   the punctuation a model wraps an id in) and looks like one: see
///   [`is_run_id`].
pub(super) fn claimed_run_ids(answer: &str) -> Vec<String> {
    let haystack = normalized(answer);
    // The cheapest of the two conditions, and the one that rules out every
    // status relay: no start verb anywhere, nothing here is a start claim.
    let starts = occurrences(&haystack, &START_CUES);
    if starts.is_empty() {
        return Vec::new();
    }
    // "Started a background workflow …" asserts a start for the whole answer,
    // however far the id then sits from the verb.
    let subject_asserted = word_occurrences(&haystack, &START_SUBJECTS)
        .into_iter()
        .any(|at| near_any(&starts, at));

    let bytes = haystack.as_bytes();
    let mut found: Vec<String> = Vec::new();

    for cue in ID_CUES {
        let mut from = 0usize;
        while let Some(hit) = haystack[from..].find(cue) {
            let after = from + hit + cue.len();
            from = after;
            // Skip the punctuation and whitespace between the cue and the id.
            let mut i = after;
            while i < bytes.len() {
                let c = bytes[i] as char;
                if ID_LEAD_IN.contains(&c) || c == '\n' || c == '\t' {
                    i += 1;
                } else {
                    break;
                }
            }
            // The token itself.
            let start = i;
            while i < bytes.len() {
                let c = bytes[i] as char;
                if c.is_ascii_hexdigit() || c == '-' {
                    i += 1;
                } else {
                    break;
                }
            }
            // A token that is only part of a longer word ("deadbeefcafe12x")
            // is not an id.
            if i < bytes.len() && (bytes[i] as char).is_ascii_alphanumeric() {
                continue;
            }
            let token = haystack[start..i].trim_end_matches('-');
            if !is_run_id(token) {
                continue;
            }
            // The assertion has to reach this id: either the answer asserts a
            // start of a workflow/run/job, or a start verb sits beside the id.
            if !subject_asserted && !near_any(&starts, start) {
                continue;
            }
            let token = token.to_string();
            if !found.contains(&token) {
                found.push(token);
            }
        }
    }
    found
}

/// Lower-cased for scanning, with the typographic apostrophe folded to `'` so
/// "I’ve queued" reads like "i've queued".
///
/// [`str::to_ascii_lowercase`] preserves byte length, so every offset taken
/// below indexes this string and its tokens are sliced from it.
fn normalized(answer: &str) -> String {
    answer.to_ascii_lowercase().replace('\u{2019}', "'")
}

/// Byte offsets of every occurrence of every needle.
fn occurrences(haystack: &str, needles: &[&str]) -> Vec<usize> {
    let mut at = Vec::new();
    for needle in needles {
        let mut from = 0usize;
        while let Some(hit) = haystack[from..].find(needle) {
            at.push(from + hit);
            from += hit + needle.len();
        }
    }
    at
}

/// As [`occurrences`], but only where the needle is a whole word — "run" must
/// not be found inside "running" or "overrun".
fn word_occurrences(haystack: &str, words: &[&str]) -> Vec<usize> {
    let bytes = haystack.as_bytes();
    let mut at = Vec::new();
    for word in words {
        let mut from = 0usize;
        while let Some(hit) = haystack[from..].find(word) {
            let start = from + hit;
            let end = start + word.len();
            from = end;
            let open = start == 0 || !(bytes[start - 1] as char).is_ascii_alphanumeric();
            let close = end >= bytes.len() || !(bytes[end] as char).is_ascii_alphanumeric();
            if open && close {
                at.push(start);
            }
        }
    }
    at
}

/// Is any of `cues` within [`NEAR_WINDOW_BYTES`] of `at`?
fn near_any(cues: &[usize], at: usize) -> bool {
    cues.iter().any(|c| c.abs_diff(at) <= NEAR_WINDOW_BYTES)
}

/// A UUID, or a hex run of 8+ digits that is **not** a plain number.
///
/// Every decimal digit is also a hex digit, so "8+ hex digits" alone calls a
/// date (`20260919`) and a counter (`12345678`) run ids. A real short id is
/// the head of a v4 UUID, which is hex: requiring one of `a`–`f` costs the
/// one-in-a-few-hundred all-decimal short form and buys back every number an
/// answer may legitimately state.
fn is_run_id(token: &str) -> bool {
    if !token.starts_with(|c: char| c.is_ascii_hexdigit()) {
        return false;
    }
    if !token.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    if token.chars().filter(char::is_ascii_hexdigit).count() < 8 {
        return false;
    }
    is_uuid_shaped(token) || token.chars().any(|c| c.is_ascii_hexdigit() && !c.is_ascii_digit())
}

/// The canonical 8-4-4-4-12 hex form, and nothing else.
fn is_uuid_shaped(token: &str) -> bool {
    let mut groups = token.split('-');
    for len in [8usize, 4, 4, 4, 12] {
        match groups.next() {
            Some(g) if g.len() == len && g.chars().all(|c| c.is_ascii_hexdigit()) => {}
            _ => return false,
        }
    }
    groups.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fabricated_sentence_states_an_id() {
        let answer = "Started a background workflow called \"Guanaco fiber notes\" \
                      (task id: `9f4c2b71`) — it will post its results here.";
        assert_eq!(claimed_run_ids(answer), vec!["9f4c2b71".to_string()]);
    }

    #[test]
    fn a_full_uuid_after_any_spelling_of_the_cue_is_found() {
        for answer in [
            "Started it (task id: 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b).",
            "Started it. task_id=9F4C2B71-1BC2-4A3D-8E55-0C1D2E3F4A5B",
            "I've queued it. Run id 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b is live.",
            "Task **9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b** started.",
        ] {
            assert_eq!(
                claimed_run_ids(answer),
                vec!["9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b".to_string()],
                "missed the id in: {answer}"
            );
        }
    }

    #[test]
    fn ordinary_prose_states_no_id() {
        for answer in [
            "I'll get that started for you.",
            "Your task list is empty.",
            "The task failed because the file was missing.",
            "That took 12345 ms and cost $0.0012.",
            "Task 3 of 7 is the slow one.",
            "A guanaco's fibre is finer than a llama's.",
            "See commit deadbeefcafe12x for the fix.",
        ] {
            assert!(
                claimed_run_ids(answer).is_empty(),
                "false positive in: {answer} -> {:?}",
                claimed_run_ids(answer)
            );
        }
    }

    /// J2 — the three sentences the round-13 re-review found the guard
    /// rejecting. None of them is a claim.
    #[test]
    fn the_reviewers_false_positives_are_not_claims() {
        for answer in [
            "Task 20260919 is the daily digest job.",
            "Run id 12345678 is just a counter.",
            "Task #1a2b3c4d is the accent colour in the palette.",
        ] {
            assert!(
                claimed_run_ids(answer).is_empty(),
                "false positive in: {answer} -> {:?}",
                claimed_run_ids(answer)
            );
        }
    }

    /// J2, shape — the two numbers stay non-ids even when a start really is
    /// asserted of them, because a plain number is never a run id.
    #[test]
    fn a_date_and_a_counter_are_never_ids() {
        for answer in [
            "I started the digest job. Task 20260919 is the one that runs nightly.",
            "I launched it: run id 12345678 is just a counter, not an id.",
        ] {
            assert!(
                claimed_run_ids(answer).is_empty(),
                "false positive in: {answer} -> {:?}",
                claimed_run_ids(answer)
            );
        }
    }

    /// The colour differs from a claim only in the assertion: a hex token in
    /// an id position IS a claim once a start is asserted beside it. Pinned so
    /// the boundary is visible rather than accidental.
    #[test]
    fn a_hex_token_beside_a_start_verb_is_still_a_claim() {
        assert_eq!(
            claimed_run_ids("Started it — task #1a2b3c4d."),
            vec!["1a2b3c4d".to_string()]
        );
    }

    /// J2, assertion — relaying what a run is doing claims no start.
    #[test]
    fn a_status_relay_asserts_nothing_and_is_not_a_claim() {
        for answer in [
            "Task 9f4c2b71 finished — it wrote two artifacts.",
            "Your workflow 372e0f11 is still running.",
            "Task id 9f4c2b71 is still running in the background.",
            "task_id=9f4c2b71 failed after four rounds.",
            "The run with task id 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b is paused.",
        ] {
            assert!(
                claimed_run_ids(answer).is_empty(),
                "a status relay was read as a claim: {answer} -> {:?}",
                claimed_run_ids(answer)
            );
        }
    }

    /// …and the same sentence with a start asserted of it is a claim again.
    #[test]
    fn the_same_id_with_a_start_verb_is_a_claim() {
        assert_eq!(
            claimed_run_ids("I've queued it — task id 9f4c2b71 — and it is now running."),
            vec!["9f4c2b71".to_string()]
        );
        assert_eq!(
            claimed_run_ids("Your workflow is now running in the background (task id: 9f4c2b71)."),
            vec!["9f4c2b71".to_string()]
        );
    }

    /// A start verb far away from both the id and any workflow/run/job word
    /// does not turn a relay into a claim.
    #[test]
    fn a_distant_start_verb_does_not_reach_the_id() {
        let answer = format!(
            "I started reading the guanaco notes you saved. {}Task 9f4c2b71 finished earlier.",
            "Their fibre is about sixteen microns across, finer than a llama's. ".repeat(3),
        );
        assert!(
            claimed_run_ids(&answer).is_empty(),
            "reached too far: {:?}",
            claimed_run_ids(&answer)
        );
    }

    #[test]
    fn two_stated_ids_are_both_reported_once() {
        let answer = "Started two: task id 9f4c2b71 and task id aabbccdd, plus task id 9f4c2b71.";
        assert_eq!(
            claimed_run_ids(answer),
            vec!["9f4c2b71".to_string(), "aabbccdd".to_string()]
        );
    }

    #[test]
    fn a_typographic_apostrophe_reads_like_a_plain_one() {
        assert_eq!(
            claimed_run_ids("I\u{2019}ve queued it (task id: 9f4c2b71)."),
            vec!["9f4c2b71".to_string()]
        );
    }

    #[test]
    fn uuid_shape_is_the_canonical_five_groups() {
        assert!(is_uuid_shaped("9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b"));
        assert!(!is_uuid_shaped("9f4c2b71-1bc2-4a3d-8e55"));
        assert!(!is_uuid_shaped("9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b-extra"));
        assert!(!is_uuid_shaped("9f4c2b71"));
    }
}

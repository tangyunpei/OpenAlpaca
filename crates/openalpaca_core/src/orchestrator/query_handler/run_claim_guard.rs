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
//! finished answer, and — narrowly — refuses one that states a run id no run
//! answers to.
//!
//! **Narrow on purpose.** The signal is a task/run id, not a turn of phrase: an
//! answer that says "I'll get that started" states nothing checkable and is
//! left alone, and an answer quoting a run that *does* exist on this lane is
//! left alone too. A turn that stated no id pays one string scan and touches no
//! database.

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
    lane_key: String,
}

impl RunClaimGuard {
    pub(super) fn new(
        start_workflow: Arc<StartWorkflowTool>,
        db: Option<Database>,
        lane_key: &str,
    ) -> Self {
        Self {
            start_workflow,
            db,
            lane_key: lane_key.to_string(),
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
        // Cheapest possible on the ordinary turn: no id stated, nothing to do,
        // no query.
        let claimed = claimed_run_ids(answer);
        if claimed.is_empty() {
            return None;
        }
        let db = self.db.as_ref()?;
        let repo = TaskRepository::new(db);
        for id in &claimed {
            match repo.lane_has_task_id_prefix(&self.lane_key, id) {
                // Quoting a run that exists is ordinary conversation.
                Ok(true) => continue,
                Ok(false) => {
                    tracing::warn!(
                        lane_key = %self.lane_key,
                        claimed_id = %id,
                        "Main-loop answer states a run id that matches no task on this lane"
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

/// Ids an answer presents as a task/run id, lowercased and deduplicated.
///
/// A candidate is a UUID or a hex run of at least 8 digits (the short form the
/// GUI and the CLI both print), optionally hyphenated — long enough that
/// ordinary prose, a version number or a count cannot trip it.
pub(super) fn claimed_run_ids(answer: &str) -> Vec<String> {
    let haystack = answer.to_ascii_lowercase();
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
            let token = &haystack[start..i];
            if !is_run_id(token) {
                continue;
            }
            // A token that is only part of a longer word ("deadbeefcafe12x")
            // is not an id.
            if i < bytes.len() && (bytes[i] as char).is_ascii_alphanumeric() {
                continue;
            }
            let token = token.trim_end_matches('-').to_string();
            if !found.contains(&token) {
                found.push(token);
            }
        }
    }
    found
}

/// A UUID, or a hyphen-free-enough hex run of 8+ digits.
fn is_run_id(token: &str) -> bool {
    let hex_digits = token.chars().filter(char::is_ascii_hexdigit).count();
    hex_digits >= 8
        && token.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
        && token.starts_with(|c: char| c.is_ascii_hexdigit())
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
            "Run id 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b is live.",
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

    #[test]
    fn two_stated_ids_are_both_reported_once() {
        let answer = "Started two: task id 9f4c2b71 and task id aabbccdd, plus task id 9f4c2b71.";
        assert_eq!(
            claimed_run_ids(answer),
            vec!["9f4c2b71".to_string(), "aabbccdd".to_string()]
        );
    }
}

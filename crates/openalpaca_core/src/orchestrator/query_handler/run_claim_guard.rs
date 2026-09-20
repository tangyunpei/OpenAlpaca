//! H3 — a chat turn never states a task id that answers to nothing.
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
//! finished answer before it becomes the turn's content.
//!
//! **The trigger is a fact, not a reading of the sentence (N1).** Rounds 13
//! and 14 tried to decide from prose whether the model was *claiming a start*;
//! both directions failed at once — an unrelated "Started reviewing your
//! notes… task 1a2b3c9d already finished" read as a claim, while ten natural
//! ways to announce a start slipped through. Guessing intent is the wrong
//! tool. What is left is checkable:
//!
//! 1. the turn's own `start_workflow` cell is empty (it really did not
//!    delegate — a turn that did is skipped, its id is on `delegation`);
//! 2. the answer states a token in an **id position** (after a `task id` /
//!    `run id` / `task` cue and the punctuation a model wraps an id in) that
//!    **looks like a run id** — a UUID, or an 8+ hex run that is not a plain
//!    number, so a date, a counter and a bare number are never ids;
//! 3. that token matches **no task of this turn's owner** — `created_by`, not
//!    the lane (J1): `task_status` answers about every run this owner started,
//!    so relaying one lane's run into another is true and must be left alone.
//!
//! Stating an id nothing answers to is an error in every case — a fabricated
//! start and a mis-recalled status alike — so the corrective round is never
//! wasted on a truthful answer, and the consequence is proportionate: one
//! corrective round, and then a runtime-authored line **appended** to whatever
//! the model said (N3). The answer is never taken away. A turn that stated no
//! id pays one string scan and touches no database.

use std::sync::Arc;

use openalpaca_storage::{Database, repository::TaskRepository};

use crate::runner::{AnswerGuard, Correction};
use crate::tools::builtins::StartWorkflowTool;

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
        // Cheapest possible on the ordinary turn: nothing stated, nothing to
        // do, no query.
        let stated = stated_run_ids(answer);
        if stated.is_empty() {
            return None;
        }
        let db = self.db.as_ref()?;
        let repo = TaskRepository::new(db);
        let mut unknown: Vec<String> = Vec::new();
        for id in &stated {
            match repo.owner_has_task_id_prefix(&self.created_by, id) {
                // Quoting a run that exists is ordinary conversation.
                Ok(true) => continue,
                Ok(false) => unknown.push(id.clone()),
                // A failed lookup must not invent a lie: say nothing at all,
                // about any of the ids.
                Err(e) => {
                    tracing::warn!(
                        lane_key = %self.lane_key,
                        "Run-claim guard could not check a stated task id: {e}"
                    );
                    return None;
                }
            }
        }
        if unknown.is_empty() {
            return None;
        }
        tracing::warn!(
            lane_key = %self.lane_key,
            claimed_ids = %unknown.join(", "),
            "Main-loop answer states a task id that matches no task of this owner"
        );
        Some(Correction {
            note: corrective_note(&unknown),
            runtime_note: runtime_note(&unknown),
        })
    }
}

/// What the model is told, once, about the id(s) it stated (N2).
///
/// True whether it fabricated a start or mis-recalled an id, which is why it
/// names both ways out instead of assuming which happened.
fn corrective_note(ids: &[String]) -> String {
    let (subject, those, them) = if ids.len() == 1 {
        ("task id", "that id", "the id")
    } else {
        ("task ids", "those ids", "them")
    };
    format!(
        "Your answer states {subject} {}, but no task of this user has {those}, and no \
         start_workflow call was made in this turn. If a run was meant to be started, call \
         start_workflow now; otherwise correct or remove {them}.",
        join_and(ids)
    )
}

/// What the reader sees appended beneath the answer if the id is still there
/// after the corrective round (N3).
///
/// Two verifiable facts and one instruction — never a denial of what the model
/// said, because the guard cannot know whether the sentence was a fabricated
/// start or a mis-remembered status.
fn runtime_note(ids: &[String]) -> String {
    let (noun, verb) = if ids.len() == 1 {
        ("no task with id", "exists")
    } else {
        ("no tasks with ids", "exist")
    };
    format!(
        "Note from OpenAlpaca: no workflow was started in this turn, and {noun} {} {verb}. \
         Ask again to start one.",
        join_and(ids)
    )
}

/// "a", "a and b", "a, b and c".
fn join_and(ids: &[String]) -> String {
    match ids {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
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

/// Ids an answer states in an id position, lowercased and deduplicated.
///
/// One condition, checkable without reading the sentence (N1): the token sits
/// after an [`ID_CUES`] cue and the punctuation a model wraps an id in, and it
/// looks like a run id ([`is_run_id`]). Whether the surrounding prose *means*
/// to claim a start is not asked — the guard's consequence is a fact appended
/// to the answer, so a broad trigger is harmless and a narrow one is not.
pub(super) fn stated_run_ids(answer: &str) -> Vec<String> {
    let haystack = normalized(answer);
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
            let token = token.to_string();
            if !found.contains(&token) {
                found.push(token);
            }
        }
    }
    found
}

/// Lower-cased for scanning.
///
/// [`str::to_ascii_lowercase`] maps each byte to one byte, so every offset
/// taken below indexes this string and the tokens sliced out of it are the
/// answer's own bytes, lowercased. Nothing else is folded — an earlier
/// apostrophe fold existed only for the start-phrase list N1 deleted, and it
/// *shrank* the string, which would have made these offsets a lie.
fn normalized(answer: &str) -> String {
    answer.to_ascii_lowercase()
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
        assert_eq!(stated_run_ids(answer), vec!["9f4c2b71".to_string()]);
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
                stated_run_ids(answer),
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
                stated_run_ids(answer).is_empty(),
                "false positive in: {answer} -> {:?}",
                stated_run_ids(answer)
            );
        }
    }

    /// The shape rule, which N1 keeps: a plain number in an id position is
    /// never an id, however the sentence introduces it.
    #[test]
    fn a_date_and_a_counter_are_never_ids() {
        for answer in [
            "Task 20260919 is the daily digest job.",
            "Run id 12345678 is just a counter.",
            "I started the digest job. Task 20260919 is the one that runs nightly.",
            "I launched it: run id 12345678 is just a counter, not an id.",
        ] {
            assert!(
                stated_run_ids(answer).is_empty(),
                "false positive in: {answer} -> {:?}",
                stated_run_ids(answer)
            );
        }
    }

    /// N1 — a hex token in an id position is stated as an id whatever the
    /// sentence is doing with it. The colour is a false positive by design:
    /// its cost is one corrective round and, at worst, a true line appended.
    #[test]
    fn a_hex_token_in_an_id_position_is_stated() {
        assert_eq!(
            stated_run_ids("Task #1a2b3c4d is the accent colour in the palette."),
            vec!["1a2b3c4d".to_string()]
        );
    }

    /// N1 — the ten phrasings the round-14 re-review walked through the old
    /// start-verb list. No verb list is consulted any more, so all ten are
    /// caught by the same rule as the literal one.
    #[test]
    fn every_way_of_announcing_a_start_is_caught() {
        const ID: &str = "9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b";
        for answer in [
            "I've kicked it off (task id: 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b).",
            "It's running in the background now — task id 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b.",
            "I fired off the workflow; task_id=9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b.",
            "Off it goes. Task 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b will report back.",
            "Consider it handled — run id 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b.",
            "That's under way: task id `9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b`.",
            "I've set a background job going, task id 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b.",
            "On it! task-id 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b",
            "Handed it to a subagent — task 9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b.",
            "Your notes are being written up now (task id: \
             9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b).",
        ] {
            assert_eq!(
                stated_run_ids(answer),
                vec![ID.to_string()],
                "slipped through: {answer}"
            );
        }
    }

    /// N1 — a status relay states an id like any other sentence. Round 14
    /// exempted these; the exemption is what let a fabricated status through.
    #[test]
    fn a_status_relay_states_its_id_too() {
        for answer in [
            "Task 9f4c2b71 finished — it wrote two artifacts.",
            "Task id 9f4c2b71 is still running in the background.",
            "task_id=9f4c2b71 failed after four rounds.",
        ] {
            assert_eq!(
                stated_run_ids(answer),
                vec!["9f4c2b71".to_string()],
                "missed the id in: {answer}"
            );
        }
    }

    #[test]
    fn two_stated_ids_are_both_reported_once() {
        let answer = "Started two: task id 9f4c2b71 and task id aabbccdd, plus task id 9f4c2b71.";
        assert_eq!(
            stated_run_ids(answer),
            vec!["9f4c2b71".to_string(), "aabbccdd".to_string()]
        );
    }

    #[test]
    fn uuid_shape_is_the_canonical_five_groups() {
        assert!(is_uuid_shaped("9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b"));
        assert!(!is_uuid_shaped("9f4c2b71-1bc2-4a3d-8e55"));
        assert!(!is_uuid_shaped("9f4c2b71-1bc2-4a3d-8e55-0c1d2e3f4a5b-extra"));
        assert!(!is_uuid_shaped("9f4c2b71"));
    }

    /// N2/N3 — both sentences are true in both cases the guard can fire on,
    /// and both name the id.
    #[test]
    fn both_notes_name_the_id_and_state_only_facts() {
        let one = vec!["9f4c2b71".to_string()];
        let note = corrective_note(&one);
        assert_eq!(
            note,
            "Your answer states task id 9f4c2b71, but no task of this user has that id, and no \
             start_workflow call was made in this turn. If a run was meant to be started, call \
             start_workflow now; otherwise correct or remove the id."
        );
        assert_eq!(
            runtime_note(&one),
            "Note from OpenAlpaca: no workflow was started in this turn, and no task with id \
             9f4c2b71 exists. Ask again to start one."
        );

        let two = vec!["9f4c2b71".to_string(), "aabbccdd".to_string()];
        assert!(corrective_note(&two).contains("task ids 9f4c2b71 and aabbccdd"));
        let plural = runtime_note(&two);
        assert!(
            plural.contains("no tasks with ids 9f4c2b71 and aabbccdd exist"),
            "plural reads wrong: {plural}"
        );
    }

    #[test]
    fn three_ids_read_as_a_list() {
        let three = ["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(join_and(&three), "a, b and c");
    }
}

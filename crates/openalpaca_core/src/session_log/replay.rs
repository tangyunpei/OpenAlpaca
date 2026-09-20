//! §5.6(c) — rebuilding an interrupted run's loop history from its log.
//!
//! The plan's one speculative piece. A run whose daemon died mid-flight left
//! its whole conversation on disk — the `round` records carry the assistant's
//! text and its `tool_use` blocks *verbatim* (§5.5's own reason for writing
//! them that way), and the `tool_result` records carry what came back. That is
//! enough to hand a fresh loop the history the dead one had, so the model
//! continues instead of starting over.
//!
//! Three rules, and each one is a promise this module keeps:
//!
//! * **Nothing is executed.** A replay re-primes context; the recorded calls
//!   come back as *messages*, never as dispatches. `rebuild` takes a directory
//!   and a task id and touches no registry, no sandbox and no network —
//!   pinned by `rebuilding_executes_no_tool`.
//! * **A round is complete or it is not replayed.** §5.6c: "stop at the
//!   last *complete* round (all its results present — the model re-does at
//!   most one round)". A crash between a `tool_call` and its `tool_result` is
//!   exactly a round whose results are missing, and half a round is not a
//!   round: an assistant message holding a `tool_use` with no answering
//!   `tool_result` is a malformed request to every provider. Such a round is
//!   dropped **where it stands**, not by truncating the history at it — a
//!   resumed run appends to the same log under the same id, so an earlier
//!   incarnation's tear sits in the middle of it (R67), closed by the `resume`
//!   record that followed. What was dropped is counted, never implied.
//! * **A compaction is honoured, not undone.** Replaying every round of a log
//!   whose live loop had already compacted would re-inflate the context that
//!   compaction shrank. The newest `compaction` record's `preserved_from_seq`
//!   is the boundary — and per T41's hand-off it is *the last seq written
//!   before the compaction record*, **not** the boundary of the tail
//!   compaction retained, so the retained tail is added back explicitly
//!   ([`rebuild`]'s `tail_keep`). What is left out is announced in one
//!   `context_compacted` note rather than summarised: the `compaction` record
//!   carries counts, not the summary text, and inventing one would be worse
//!   than saying what happened.

use super::reader::{LoggedRecord, read_records_after_torn};
use super::record::RESULTS_DIR;
use super::spill_stub;
use chrono::{DateTime, Utc};
use openalpaca_llm::{ChatMessage, ToolCall};
use serde_json::Value;
use std::collections::HashMap;
use std::io;
use std::path::Path;

/// Records read per page. The same bound `recovery` uses, for the same
/// reason: a session may hold `log_max_session_bytes` and must never be
/// materialised whole.
const PAGE: usize = 512;

/// What a replay rebuilt, and what it had to leave out.
///
/// Every count is reported rather than logged away: the `resume` record the
/// caller writes into the log is built from this, so the next reader can see
/// exactly which slice of the transcript the resumed run was given.
#[derive(Debug, Clone, Default)]
pub struct ReplayPlan {
    /// The rebuilt history, to be spliced in **after** the caller's system
    /// prompt and objective message (§5.6c composes those fresh).
    pub messages: Vec<ChatMessage>,
    /// Complete rounds replayed.
    pub rounds: usize,
    /// Tool results replayed.
    pub tool_results: usize,
    /// Lowest / highest log seq consumed — the "source seq range".
    pub from_seq: Option<u64>,
    pub to_seq: Option<u64>,
    /// The `preserved_from_seq` of the newest `compaction` record honoured.
    pub compacted_from_seq: Option<u64>,
    /// Rounds dropped ahead of the compaction boundary and its retained tail.
    pub compacted_rounds_dropped: usize,
    /// Incomplete rounds dropped: the crash's own torn round (the one the
    /// model re-does), plus any earlier incarnation's tear and any round a
    /// dropped `tool_result` record left permanently half-answered. Counted
    /// rather than flagged so the `resume` record **states** the gap instead
    /// of leaving it to be inferred from a round count (R67).
    pub dropped_incomplete_rounds: usize,
    /// The seq the surviving history starts at, when the log is **not whole**
    /// at its head: the oldest segments were evicted by the per-session byte
    /// cap, or the records before this one are otherwise gone. `None` means
    /// nothing was lost from the head — not that nothing was lost at all (see
    /// [`ReplayPlan::trim_reason`], which a torn record also sets).
    pub trimmed_from_seq: Option<u64>,
    /// Why part of the log is missing, in one clause. Present exactly when the
    /// rebuild found evidence the log is not whole; it is what the head note
    /// and the `resume` record both say.
    pub trim_reason: Option<String>,
    /// `tool_result`s whose payload lives in `results/`.
    pub spills_referenced: usize,
    /// …of which the file is gone (the sweep took it): the preview is
    /// replayed without the promise of a page.
    pub missing_spills: usize,
    /// The timestamp of the run's last record — when the run stopped, as far
    /// as anything on disk knows.
    pub last_ts: Option<DateTime<Utc>>,
}

impl ReplayPlan {
    /// Whether there is anything to resume from. `true` is §5.6c's "gutted
    /// log", which the launch verb answers as a clean refusal pointing at
    /// `rerun`.
    ///
    /// It is the **rounds** that decide, not the message count: the notes this
    /// rebuild puts at the head (a trimmed log, a compaction) describe a
    /// history, and describing one that is not there would resume a run over
    /// nothing but an apology.
    pub fn is_empty(&self) -> bool {
        self.rounds == 0
    }
}

/// The marker that turns a launch into a replay resume.
///
/// It is carried through `dispatch_lead_agent_resume` rather than re-derived
/// in the runner for two reasons: the refusal for a gutted log has to happen
/// *before* anything is claimed or dispatched (so the row survives it), and
/// the session to replay is the run's own — not the one the lane happens to
/// be showing when the resume is asked for.
#[derive(Debug, Clone)]
pub struct ResumeSeed {
    pub session_id: String,
    pub replay: ReplayPlan,
}

/// What the lead's runner is handed for a resume.
///
/// The plan is carried whole (not just its messages) so the `resume` record
/// the runner writes into the log can name the slice it was built from.
#[derive(Debug, Clone)]
pub struct ResumeHistory {
    pub plan: ReplayPlan,
    /// The synthetic interjection — present **only** when the steering rail
    /// could not carry it (steering disabled, or a push the inbox refused).
    /// The rebuilt history then carries the identical `<user_interjection>`
    /// text as its last message instead, so the model reads the same thing
    /// either way.
    pub inline_note: Option<String>,
}

/// The synthetic interjection §5.6c appends after the rebuilt history.
///
/// One sentence of fact and one instruction, and the instruction is the
/// important half: side effects between the last durable record and the crash
/// are unknowable, so the model is told to treat the recorded calls as done
/// rather than being left to guess. It goes in through the steering rail, so
/// the loop sees it as a `<user_interjection>` — the channel the model's
/// prompt already teaches it to obey mid-run.
pub fn resume_interjection(interrupted_at: DateTime<Utc>) -> String {
    format!(
        "This run was interrupted at {}; continue from the last completed step. \
         Do not repeat side-effecting tool calls already recorded.",
        interrupted_at.to_rfc3339()
    )
}

/// One round as the log recorded it, with the results that answered it.
struct Round {
    seq: u64,
    text: String,
    calls: Vec<ToolCall>,
    /// One entry per call, in call order — `None` until its result is found.
    /// The seq travels with the message so the plan's reported range covers
    /// the results as well as the rounds that asked for them.
    results: Vec<Option<(u64, ChatMessage)>>,
}

impl Round {
    fn complete(&self) -> bool {
        self.results.iter().all(Option::is_some)
    }
}

/// Rebuild `task_id`'s loop history from the session log in `session_dir`.
///
/// `tail_keep` is the loop's own `context_tail_keep` — the number of rounds a
/// compaction leaves in place — and is what the replay adds back ahead of a
/// compaction boundary (see the module doc).
///
/// A missing directory, a torn tail and a log holding nothing for this run are
/// all the same answer: an empty plan. None of them is an error; the caller
/// decides what an empty plan means.
pub fn rebuild(session_dir: &Path, task_id: &str, tail_keep: usize) -> io::Result<ReplayPlan> {
    let mut plan = ReplayPlan::default();

    let mut rounds: Vec<Round> = Vec::new();
    // tool_use_id → (round index, call index), so a result finds its slot in
    // one lookup however far it landed from the round that asked for it.
    let mut slots: HashMap<String, (usize, usize)> = HashMap::new();
    let mut compaction: Option<(u64, u64)> = None; // (record seq, preserved_from_seq)

    // What the log itself says it lost, and what the rebuild noticed: a
    // trimmed head is invisible from the records that survived it, so the
    // evidence is gathered as the pages are walked (see `trim_reason`).
    let mut first_seq: Option<u64> = None;
    let mut trimmed_range: Option<(u64, u64)> = None;
    let mut orphan_results = 0usize;
    let mut torn_record = false;

    let mut cursor: Option<u64> = None;
    loop {
        let (page, torn) = read_records_after_torn(session_dir, cursor, PAGE)?;
        torn_record |= torn;
        if page.is_empty() {
            break;
        }
        cursor = page.last().map(|r| r.seq);
        let short = page.len() < PAGE;
        for record in &page {
            if first_seq.is_none() {
                first_seq = Some(record.seq);
            }
            // §5.4's trim notice carries no task id — it is a fact about the
            // session's log, not about one run in it — so it is read before
            // the run filter, which is why the first cut of this module never
            // saw it.
            if record.kind == "log_trimmed" {
                let from = record.data.get("from_seq").and_then(Value::as_u64);
                let to = record.data.get("to_seq").and_then(Value::as_u64);
                if let (Some(from), Some(to)) = (from, to) {
                    trimmed_range = Some(match trimmed_range {
                        Some((lo, hi)) => (lo.min(from), hi.max(to)),
                        None => (from, to),
                    });
                }
            }
            if record.task_id.as_deref() != Some(task_id) {
                continue;
            }
            plan.last_ts = Some(record.ts);
            match record.kind.as_str() {
                "round" => {
                    let calls = tool_calls_of(&record.data);
                    let index = rounds.len();
                    for (slot, call) in calls.iter().enumerate() {
                        slots.insert(call.id.clone(), (index, slot));
                    }
                    rounds.push(Round {
                        seq: record.seq,
                        text: text_of(&record.data),
                        results: vec![None; calls.len()],
                        calls,
                    });
                }
                "tool_result" => {
                    let Some(id) = record.data.get("tool_use_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(&(round, slot)) = slots.get(id) else {
                        // A result whose round is not in the log — its segment
                        // was rotated away. Nothing to attach it to, and proof
                        // that rounds of this run are missing.
                        orphan_results += 1;
                        continue;
                    };
                    let content = result_text(session_dir, record, &mut plan);
                    rounds[round].results[slot] =
                        Some((record.seq, ChatMessage::tool_result(id, &content)));
                }
                "compaction" => {
                    if let Some(preserved) =
                        record.data.get("preserved_from_seq").and_then(Value::as_u64)
                    {
                        compaction = Some((record.seq, preserved));
                    }
                }
                // R67: a `resume` record is a round boundary. Everything
                // before it belongs to an incarnation that has already ended,
                // so its last round can never be answered now — and the
                // results that follow belong to the run that re-entered here.
                // Clearing the slots keeps a later result from ever attaching
                // to a closed incarnation's call.
                "resume" => slots.clear(),
                _ => {}
            }
        }
        if short {
            break;
        }
    }

    // §5.6c: only a **complete** round is replayed — an assistant message
    // holding a `tool_use` with no answering `tool_result` is a malformed
    // request to every provider.
    //
    // Each round is dropped where it stands rather than truncating the history
    // at the first one (R67). Truncating is right exactly once: a resumed run
    // appends to *this* log under *this* id, so the round the last crash tore
    // is an interior incomplete round the moment a second resume reads it, and
    // cutting there would throw away every round the resumed run completed —
    // then tell the model not to repeat side effects it can no longer see. A
    // round is self-contained (its assistant message and its partial results
    // leave together), so dropping in place keeps the provider invariant.
    let before = rounds.len();
    rounds.retain(Round::complete);
    plan.dropped_incomplete_rounds = before - rounds.len();

    // The compaction boundary, plus the tail compaction kept (T41's hand-off:
    // `preserved_from_seq` is not that tail's boundary).
    if let Some((_, preserved)) = compaction {
        plan.compacted_from_seq = Some(preserved);
        let after = rounds.iter().filter(|r| r.seq > preserved).count();
        let keep = after + tail_keep.min(rounds.len() - after);
        plan.compacted_rounds_dropped = rounds.len() - keep;
        rounds.drain(..plan.compacted_rounds_dropped);
    }

    // What the log lost before this rebuild ever saw it (§5.4's trim, a torn
    // record), named at the head of the history the same way a compaction's
    // gap is. Without it the model is handed rounds 20–40 as if they were the
    // whole run and told to continue — an invitation to re-do the side effects
    // of rounds 1–19.
    (plan.trimmed_from_seq, plan.trim_reason) = trim_evidence(
        first_seq,
        trimmed_range,
        orphan_results,
        torn_record,
    );
    // A note with no history behind it is not a history: a run whose rounds
    // are all gone is `is_empty`, which the launch verb answers as the clean
    // refusal pointing at `rerun`.
    if let Some(ref reason) = plan.trim_reason
        && !rounds.is_empty()
    {
        plan.messages
            .push(ChatMessage::user(&trim_note(plan.trimmed_from_seq, reason)));
    }
    if plan.compacted_rounds_dropped > 0 {
        plan.messages.push(ChatMessage::user(&compaction_note(
            plan.compacted_rounds_dropped,
        )));
    }
    let mut span: Option<(u64, u64)> = None;
    for round in rounds {
        widen(&mut span, round.seq);
        plan.rounds += 1;
        plan.messages.push(assistant_message(&round));
        for (seq, result) in round.results.into_iter().flatten() {
            widen(&mut span, seq);
            plan.tool_results += 1;
            plan.messages.push(result);
        }
    }
    if let Some((from, to)) = span {
        plan.from_seq = Some(from);
        plan.to_seq = Some(to);
    }

    Ok(plan)
}

/// Grow the `(lowest, highest)` seq the replay consumed.
fn widen(span: &mut Option<(u64, u64)>, seq: u64) {
    *span = Some(match *span {
        Some((lo, hi)) => (lo.min(seq), hi.max(seq)),
        None => (seq, seq),
    });
}

/// The `round` record's assistant message: its text plus its `tool_use`
/// blocks verbatim, exactly as `ChatMessage::assistant_with_tools` built it
/// the first time.
fn assistant_message(round: &Round) -> ChatMessage {
    ChatMessage {
        role: openalpaca_llm::Role::Assistant,
        content: round.text.clone(),
        parts: None,
        tool_calls: (!round.calls.is_empty()).then(|| round.calls.clone()),
        tool_call_id: None,
    }
}

fn text_of(data: &Value) -> String {
    data.get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn tool_calls_of(data: &Value) -> Vec<ToolCall> {
    data.get("tool_use")
        .and_then(Value::as_array)
        .map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    Some(ToolCall {
                        id: call.get("id").and_then(Value::as_str)?.to_string(),
                        name: call.get("name").and_then(Value::as_str)?.to_string(),
                        arguments: call.get("input").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// What the model is handed for one recorded result.
///
/// A result that sat inline comes back as itself. A **spilled** one comes back
/// as [`spill_stub`] — byte-for-byte the text the loop handed the model when
/// the call first ran, because the payload never entered the context the first
/// time and re-priming must not put it there now. When the sweep has since
/// taken the `results/` file the stub would promise a page that no longer
/// exists, so the preview is replayed with that promise removed instead.
fn result_text(session_dir: &Path, record: &LoggedRecord, plan: &mut ReplayPlan) -> String {
    match record.data.get("result") {
        Some(Value::String(inline)) => inline.clone(),
        Some(Value::Object(spilled)) => {
            plan.spills_referenced += 1;
            let preview = spilled
                .get("preview")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let spill = spilled.get("spill");
            let rel = spill
                .and_then(|s| s.get("rel"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            let bytes = spill
                .and_then(|s| s.get("bytes"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            if rel.is_empty() || !spill_file_present(session_dir, rel) {
                plan.missing_spills += 1;
                return format!(
                    "[result too large: {bytes} bytes; first 2 KB follow]\n{preview}\n\
                     [the full result was removed by the session-log sweep and cannot be paged]"
                );
            }
            spill_stub(bytes, preview, rel)
        }
        // No `result` key at all: a record from a build that did not write one,
        // or one the envelope cap collapsed to a stub. The call still happened
        // and the model must be told so rather than shown a hole.
        _ => "[tool result unavailable — the session log did not record it]".to_string(),
    }
}

/// Whether `rel` (`results/<name>`) still exists under this session.
///
/// The grammar is deliberately as narrow as `read_result`'s: one file name
/// directly under `results/`, so a reference read back off disk can never walk
/// out of the session directory.
fn spill_file_present(session_dir: &Path, rel: &str) -> bool {
    let Some(name) = rel.strip_prefix(RESULTS_DIR).and_then(|r| r.strip_prefix('/')) else {
        return false;
    };
    if name.is_empty()
        || name.starts_with('.')
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return false;
    }
    session_dir.join(RESULTS_DIR).join(name).is_file()
}

/// What the log says — and what the rebuild noticed — about its own gaps.
///
/// Returns the seq the surviving history starts at (only when the **head** is
/// gone: a log always starts at seq 1, so a first record above it means
/// records were removed) and the one-clause reason the note and the `resume`
/// record both carry. `(None, None)` is a whole log.
fn trim_evidence(
    first_seq: Option<u64>,
    trimmed_range: Option<(u64, u64)>,
    orphan_results: usize,
    torn_record: bool,
) -> (Option<u64>, Option<String>) {
    let head_lost = first_seq.is_some_and(|seq| seq > 1) || trimmed_range.is_some();
    let mut reasons: Vec<String> = Vec::new();
    match trimmed_range {
        // The trim notice survived the trim, so the range it took is known.
        Some((from, to)) => reasons.push(format!(
            "the session log's oldest segments were removed by its size cap (seq {from}–{to})"
        )),
        None if head_lost => {
            reasons.push("the session log's oldest records are no longer on disk".to_string())
        }
        None => {}
    }
    if orphan_results > 0 {
        reasons.push(format!(
            "{orphan_results} of this run's recorded tool results have no surviving round"
        ));
    }
    if torn_record {
        reasons.push(
            "a record was written only partially (the daemon died mid-write), so what followed it \
             could not be read"
                .to_string(),
        );
    }
    if reasons.is_empty() {
        return (None, None);
    }
    (head_lost.then_some(first_seq).flatten(), Some(reasons.join("; ")))
}

/// The one message that stands in for a history the log no longer holds.
///
/// Same contract as [`compaction_note`]: name the gap, never summarise it and
/// never paper over it. A resumed model that is shown the tail of a run as if
/// it were the whole run will re-do the side effects of the head.
fn trim_note(from_seq: Option<u64>, reason: &str) -> String {
    match from_seq {
        Some(seq) => format!(
            "<log_trimmed from_seq=\"{seq}\">Part of this run's transcript is unavailable: \
             {reason}. History before log seq {seq} is gone — earlier steps of this run are not \
             shown here, and work they recorded may already be done.</log_trimmed>"
        ),
        None => format!(
            "<log_trimmed>Part of this run's transcript is unavailable: {reason}. The steps it \
             held are not shown here, and work they recorded may already be done.</log_trimmed>"
        ),
    }
}

/// The one message that stands in for the head a compaction dropped.
///
/// Not a summary: the `compaction` record carries token and message counts
/// and no summary text (§5.4 — the summary lives in the loop's memory, which
/// died with it), and a fabricated one would be the worst kind of context.
/// Naming the gap is the honest option, and it is wrapped the way every other
/// untrusted-context block is so the model reads it as narration.
fn compaction_note(dropped: usize) -> String {
    format!(
        "<context_compacted rounds=\"{dropped}\">This conversation was compacted while it ran. \
         {dropped} earlier round{} of this run are not shown; their outcomes are reflected in \
         the steps that follow.</context_compacted>",
        if dropped == 1 { "" } else { "s" }
    )
}

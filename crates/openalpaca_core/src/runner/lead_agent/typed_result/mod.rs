//! Typed subagent results (SA-4, design §5, §5.1, §5.2, §5.3): a child
//! spawned with a `result_schema` gets one tool of its own,
//! `submit_result`, and ends by calling it. Its `result` is checked by
//! OpenAlpaca's own checker ([`shape`]); a rejection's lines go back to the
//! child as the tool's error, four submissions in all; the loop reads the
//! verdict from the run's [`TypedResultSlot`] after each tool round.

mod shape;

pub(in crate::runner::lead_agent) use shape::{ResultShape, asks_nothing};

use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolRegistry};
use openalpaca_llm::ToolDefinition;
use serde_json::Value;
use shape::ShapeVerdict;
use std::sync::{Arc, Mutex};

/// The tool's name, reserved (Y-18 (1)): a typed child's alone.
pub(crate) const SUBMIT_RESULT: &str = "submit_result";

/// Submissions in all: the first and three repairs (Y-4); the fourth
/// rejection ends the child.
const MAX_SUBMISSIONS: usize = 4;

/// What `submit_result` tells the model (Y-2, Y-13, Y-20 (7)).
const DESCRIPTION: &str = "Deliver your result: call this with the result as `result`, \
     matching this tool's schema; your final text is not the result. `format` is not \
     checked. Each call, accepted or not, counts against your tool-call limit.";

/// The header of a rejection (Y-4).
const REJECTION_HEADER: &str = "The result does not match result_schema:";

/// What an accepted call answers (Y-16); only the session log sees it.
const ACCEPTED: &str = "Result accepted.";

/// What a later call answers once a result was accepted (Y-16's words).
const ALREADY_ACCEPTED: &str = "a result was already accepted";

/// A check that could not run — a bug in the checker — spends the call
/// and is not a rejection (§5.3.4 step 3).
const CHECK_FAILED: &str = "submit_result could not check the result: the check failed inside \
     the daemon; this is not a rejection";

/// The nudge a typed child is sent once when it ends a round with text
/// and no call (Y-4, Y-10).
pub(crate) const NUDGE: &str =
    "You ended without calling submit_result. Call it now with a result matching the schema.";

/// The verdict the loop reads after a tool round (Y-9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotVerdict {
    /// Nothing decided yet.
    Open,
    /// A submission was accepted: the run ends `ResultSubmitted`.
    Accepted,
    /// The fourth rejection: the run ends `ResultRejected`.
    Rejected,
}

/// Which cap refused a `submit_result` call in the loop (Y-20 (3)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapRefusal {
    /// `max_tool_calls`, the run's cap.
    Run,
    /// `max_tools_per_round`.
    Round,
}

/// What a typed run's slot holds when its loop has returned.
#[derive(Debug, Clone, Default)]
pub(in crate::runner::lead_agent) struct SlotOutcome {
    /// The accepted value, as compact JSON.
    pub(in crate::runner::lead_agent) accepted: Option<String>,
    /// How many submissions were rejected.
    pub(in crate::runner::lead_agent) rejections: usize,
    /// The last rejection's lines, joined for the lead.
    pub(in crate::runner::lead_agent) last_violations: Option<String>,
    /// The last rejected value as compact JSON, kept only when it was
    /// within the result cap (none for a missing or over-size result).
    pub(in crate::runner::lead_agent) last_rejected: Option<String>,
    /// A `submit_result` call was refused by the run's tool-call cap.
    pub(in crate::runner::lead_agent) run_cap_refused: bool,
    /// The last round in which a `submit_result` call was refused by the
    /// per-round cap (the loop's round count, as `rounds_used` reads it).
    pub(in crate::runner::lead_agent) round_cap_refused_in: Option<usize>,
}

#[derive(Debug, Default)]
struct SlotState {
    outcome: SlotOutcome,
    /// The text the last rejection answered, which a call after the
    /// fourth is answered with again.
    last_answer: Option<String>,
}

/// One typed run's verdict, shared by its `submit_result` tool, its loop
/// (`LoopConfig::typed_result`) and the spawn that reads it after the loop
/// (Y-9). Built only by `Default`, as the spawn builds it. Written by the
/// tool only after its check has returned, never from inside the check.
#[derive(Debug, Default)]
pub struct TypedResultSlot {
    state: Mutex<SlotState>,
}

impl TypedResultSlot {
    fn lock(&self) -> std::sync::MutexGuard<'_, SlotState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// The verdict after a round: accepted, the fourth rejection, or open.
    pub(crate) fn verdict(&self) -> SlotVerdict {
        let state = self.lock();
        if state.outcome.accepted.is_some() {
            SlotVerdict::Accepted
        } else if state.outcome.rejections >= MAX_SUBMISSIONS {
            SlotVerdict::Rejected
        } else {
            SlotVerdict::Open
        }
    }

    /// Record that the loop refused a `submit_result` call for a cap in
    /// `round`: not executed, not counted, not a rejection (Y-20 (1), (3)).
    pub(crate) fn record_cap_refusal(&self, cap: CapRefusal, round: usize) {
        let mut state = self.lock();
        match cap {
            CapRefusal::Run => state.outcome.run_cap_refused = true,
            CapRefusal::Round => state.outcome.round_cap_refused_in = Some(round),
        }
    }

    /// What the slot holds, for the spawn to compose the lane's close.
    pub(in crate::runner::lead_agent) fn outcome(&self) -> SlotOutcome {
        self.lock().outcome.clone()
    }

    /// Once resolved, a later call is answered from the slot and changes
    /// nothing (Y-16).
    fn answer_if_resolved(&self) -> Option<Result<String, String>> {
        let state = self.lock();
        if state.outcome.accepted.is_some() {
            return Some(Ok(ALREADY_ACCEPTED.to_string()));
        }
        if state.outcome.rejections >= MAX_SUBMISSIONS {
            return state.last_answer.clone().map(Err);
        }
        None
    }

    fn accept(&self, compact: String) -> Result<String, String> {
        let mut state = self.lock();
        if state.outcome.accepted.is_some() {
            return Ok(ALREADY_ACCEPTED.to_string());
        }
        state.outcome.accepted = Some(compact);
        Ok(ACCEPTED.to_string())
    }

    /// Count one rejection and answer it with its lines.
    fn reject(&self, lines: &[String], more: usize, rejected: Option<String>) -> String {
        let mut answer = String::from(REJECTION_HEADER);
        for line in lines {
            answer.push('\n');
            answer.push_str(line);
        }
        let mut violations = lines.join("; ");
        if more > 0 {
            let notice = format!("… and {more} more violations");
            answer.push('\n');
            answer.push_str(&notice);
            violations.push_str("; ");
            violations.push_str(&notice);
        }
        let mut state = self.lock();
        state.outcome.rejections += 1;
        state.outcome.last_violations = Some(violations);
        state.outcome.last_rejected = rejected;
        state.last_answer = Some(answer.clone());
        answer
    }
}

/// A `JoinError` from the check's blocking task — a panic in the checker —
/// as the call's error: a spent call, not a rejection (§5.3.4 step 3).
fn check_failure(_error: tokio::task::JoinError) -> String {
    CHECK_FAILED.to_string()
}

/// What the blocking check hands back to `execute`'s async part.
enum Checked {
    Accepted(String),
    Rejected {
        lines: Vec<String>,
        more: usize,
        raw: String,
    },
    TooLarge,
}

/// The per-child tool a typed child delivers its result with (Y-2): an
/// ordinary builtin, its arguments reaching no program.
pub(in crate::runner::lead_agent) struct SubmitResultTool {
    shape: Arc<ResultShape>,
    slot: Arc<TypedResultSlot>,
}

impl BuiltInTool for SubmitResultTool {
    async fn execute(&self, arguments: &Value) -> Result<String, String> {
        if let Some(answer) = self.slot.answer_if_resolved() {
            return answer;
        }
        // A missing `result` is one rejection (Y-16): the registered
        // parameters are permissive, so it reaches here (Y-11 (2)).
        let Some(result) = arguments.get("result") else {
            return Err(self
                .slot
                .reject(&[shape::MISSING_RESULT_LINE.to_string()], 0, None));
        };
        // The size, inline, before anything is cloned (§5.3.4 step 2).
        if !shape::result_fits(result) {
            return Err(self
                .slot
                .reject(&[shape::TOO_LARGE_LINE.to_string()], 0, None));
        }
        // One clone, then off the runtime worker; the clone is dropped
        // there. The slot is written below, after the task has returned,
        // so a call the sandbox cut never lands a verdict.
        let raw = result.clone();
        let shape = Arc::clone(&self.shape);
        let checked = tokio::task::spawn_blocking(move || {
            let compact = |v: &Value| serde_json::to_string(v).unwrap_or_default();
            match shape.check(&raw) {
                ShapeVerdict::Matches { parsed } => {
                    Checked::Accepted(compact(parsed.as_ref().unwrap_or(&raw)))
                }
                ShapeVerdict::Violations { lines, more } => Checked::Rejected {
                    lines,
                    more,
                    raw: compact(&raw),
                },
                ShapeVerdict::TooLarge => Checked::TooLarge,
            }
        })
        .await
        .map_err(check_failure)?;
        match checked {
            Checked::Accepted(compact) => self.slot.accept(compact),
            Checked::Rejected { lines, more, raw } => {
                Err(self.slot.reject(&lines, more, Some(raw)))
            }
            Checked::TooLarge => {
                Err(self
                    .slot
                    .reject(&[shape::TOO_LARGE_LINE.to_string()], 0, None))
            }
        }
    }
}

/// The definition a typed child is sent (Y-2, Y-12): `result` wrapping the
/// schema, strict where strict mode accepts it (`strict_mode_compatible`
/// decides as it does for every tool).
pub(crate) fn submit_result_tool_definition(schema: &Value) -> ToolDefinition {
    ToolDefinition {
        name: SUBMIT_RESULT.to_string(),
        description: DESCRIPTION.to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {"result": schema},
            "required": ["result"],
            "additionalProperties": false
        }),
        strict: Some(true),
        input_examples: None,
    }
}

/// Register a typed child's `submit_result` on its own registry clone
/// (Y-11): permissive parameters, so a missing or wrongly typed `result`
/// reaches `execute` (R8); no capability, so nothing resolves it — the
/// child's surface gets the wrapper by hand.
pub(in crate::runner::lead_agent) fn register_submit_result(
    registry: &ToolRegistry,
    shape: Arc<ResultShape>,
    slot: Arc<TypedResultSlot>,
) {
    let registered = registry.register(RegisteredTool {
        definition: ToolDefinition {
            name: SUBMIT_RESULT.to_string(),
            description: DESCRIPTION.to_string(),
            parameters: serde_json::json!({"type": "object"}),
            strict: None,
            input_examples: None,
        },
        backend: ToolBackend::built_in(SubmitResultTool { shape, slot }),
        provides_capabilities: vec![],
        exempt_from_timeout: false,
        annotations: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    });
    // A fixed, valid name: `register` refuses only an empty, over-long or
    // NUL-holding one.
    debug_assert!(registered.is_ok(), "{registered:?}");
}

/// Admit `submit_result` for a typed child's run (Y-11): its own step
/// beside `SandboxPolicy::admit_tool_surface`, which already admits it for
/// a non-empty allow list (the definition is on the child's surface) and
/// leaves an empty one alone. The deny list is checked first: a template
/// that denies the name keeps it denied.
pub(in crate::runner::lead_agent) fn admit_submit_result(
    policy: &mut crate::security::sandbox::SandboxPolicy,
) {
    if policy
        .denied_capabilities
        .iter()
        .any(|denied| denied.eq_ignore_ascii_case(SUBMIT_RESULT))
    {
        return;
    }
    if let crate::security::capabilities::Allowlist::Only(ref mut allowed) =
        policy.allowed_capabilities
        && allowed.is_empty()
    {
        allowed.push(SUBMIT_RESULT.to_string());
    }
}

/// The text a typed failure closes its lane with (Y-15, Y-20 (3), (4)),
/// beginning at "ended without …" (the tracker adds the run id). Its fixed
/// sentences stay under the 256 bytes `wait_for_subagents`'s cut keeps at
/// each end; the violations sit between them and are cut first. The
/// per-round cap's text is said only when its refusal fell in the run's
/// last round (`rounds_used`); an earlier one leaves the plain text.
pub(in crate::runner::lead_agent) fn failure_text(
    outcome: &SlotOutcome,
    rejected_fourth: bool,
    tool_calls_made: usize,
    max_tool_calls: Option<u32>,
    rounds_used: usize,
    final_text: &str,
    run_id: &str,
) -> String {
    let n = outcome.rejections;
    let violations = outcome.last_violations.as_deref().unwrap_or_default();
    let calls_spent =
        max_tool_calls.filter(|max| outcome.run_cap_refused || tool_calls_made >= *max as usize);
    let final_text = final_text_cut(final_text);
    if rejected_fourth {
        return rejected_text(n, violations, run_id);
    }
    match calls_spent {
        Some(max) if n > 0 => format!(
            "ended without a result matching its result_schema ({n} rejected; it used all \
             {max} of its tool calls (max_tool_calls) before a fourth): {violations}. Its \
             last answer is kept: check_subagent_status {run_id}."
        ),
        Some(max) => with_final(
            format!(
                "ended without submitting a result: it used all {max} of its tool calls \
                 (max_tool_calls), and submit_result counts against them."
            ),
            &final_text,
        ),
        None if n > 0 => rejected_text(n, violations, run_id),
        None if outcome.round_cap_refused_in == Some(rounds_used) => with_final(
            "ended without submitting a result: its submit_result call was refused by the \
             per-round tool limit (max_tools_per_round) in its last round."
                .to_string(),
            &final_text,
        ),
        None => match final_text.as_str() {
            "" => "ended without submitting a result.".to_string(),
            text => format!("ended without submitting a result: {text}."),
        },
    }
}

fn rejected_text(n: usize, violations: &str, run_id: &str) -> String {
    format!(
        "ended without a result matching its result_schema ({n} rejected; at most \
         {MAX_SUBMISSIONS}): {violations}. Its last answer is kept: check_subagent_status \
         {run_id}."
    )
}

/// `sentence`, then the final text and a full stop, when there is one.
fn with_final(sentence: String, final_text: &str) -> String {
    if final_text.is_empty() {
        sentence
    } else {
        format!("{sentence} {final_text}.")
    }
}

/// How much of a child's final text a typed failure quotes.
const FINAL_TEXT_BYTES: usize = 200;

/// The final text, trimmed, cut at [`FINAL_TEXT_BYTES`] with `…` after a
/// cut; its whole form stays in the lane's output.
fn final_text_cut(text: &str) -> String {
    let text = text.trim();
    let kept = crate::utils::text::byte_prefix(text, FINAL_TEXT_BYTES);
    if kept.len() < text.len() {
        format!("{kept}…")
    } else {
        kept.to_string()
    }
}

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillExecutionEntry {
    pub id: Option<i64>,
    pub request_id: String,
    pub skill_id: String,
    pub agent_id: String,
    pub status: String,
    pub finish_reason: Option<String>,
    pub error_message: Option<String>,
    pub validation_failures: Option<String>,
    pub duration_ms: i64,
    pub rounds_used: Option<i32>,
    pub tool_calls_made: Option<i32>,
    pub input_tokens: i32,
    pub output_tokens: i32,
    pub cost_usd: f64,
    pub model_used: Option<String>,
    pub query_preview: Option<String>,
    pub route_score: Option<f64>,
    pub was_auto_selected: bool,
    pub repair_attempted: bool,
    pub repair_succeeded: bool,
    pub timestamp: Option<DateTime<Utc>>,
}

/// Maximum length, in characters, of `tool_execution_log`'s two preview
/// columns (migration 039 documents both as "≤ 2048 chars").
///
/// The row is an *index* over the session event log: the full arguments and
/// the full result live in the JSONL record `log_seq` points at, or in the
/// `results/` spill file `result_ref` names. Nothing downstream should read a
/// preview expecting the whole payload.
pub const PREVIEW_CHARS: usize = 2048;

/// One row of `tool_execution_log`.
///
/// Migration 030 created it as a plain per-tool audit row (`invocations_today`
/// on `GET /v1/tools` counts these). 039 widened it into the **tool-call index
/// of the session event log** (§5.4): `session_id`/`task_id` say which
/// transcript and which run the call belonged to, `log_seq` points at the
/// `tool_call` record that carries the full arguments, `result_ref` at the
/// record (`log:<seq>`) or spill file (`file:results/<…>`) that carries the
/// result, and the two previews let an expanded-turn view render without
/// opening either.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolExecutionEntry {
    pub id: Option<i64>,
    pub request_id: Option<String>,
    pub agent_id: String,
    pub tool_name: String,
    pub success: bool,
    pub duration_ms: i64,
    pub error_message: Option<String>,
    pub timestamp: Option<DateTime<Utc>>,
    /// The session whose event log carries this call's records (039).
    pub session_id: Option<String>,
    /// The run the call happened inside; `None` for a main-loop turn.
    pub task_id: Option<String>,
    /// `seq` of the `tool_call` record in the session's JSONL.
    pub log_seq: Option<i64>,
    /// First [`PREVIEW_CHARS`] characters of the call's arguments.
    pub args_preview: Option<String>,
    /// First [`PREVIEW_CHARS`] characters of the call's result.
    pub result_preview: Option<String>,
    /// Where the full result lives: `log:<seq>` while it sits inline in the
    /// JSONL, `file:results/<…>` once it spills.
    pub result_ref: Option<String>,
}

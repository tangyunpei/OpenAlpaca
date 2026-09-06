//! The record envelope and the event catalog (§5.4).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Envelope version. Bumped only when the envelope's own shape changes —
/// never for a new `type`, which readers must tolerate.
pub const ENVELOPE_VERSION: u8 = 1;

/// Hard cap on a record's serialised `data` object (§5.4).
///
/// It is the *envelope* bound, not the tool-result threshold: once the
/// `results/` spill lands (T42) an over-threshold result never sits inline,
/// so this cap only ever catches a payload that has no spill path.
pub const ENVELOPE_DATA_CAP_BYTES: usize = 64 * 1024;

/// Characters of an over-cap payload kept inline as its preview (§5.4's
/// "first 2048 chars" — the same bytes `tool_execution_log.result_preview`
/// stores, so the expanded-turn view renders from either).
pub const PREVIEW_CHARS: usize = 2048;

/// The event catalog (§5.4).
///
/// A reader matches on the *string*, not this enum, so an older daemon can
/// read a newer log. The enum exists for the writers, for the fsync policy,
/// and so a later sweep can name the record kinds it must recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordType {
    /// A boot boundary: written again on every daemon boot that touches the
    /// session (P-13), never once per session lifetime.
    SessionStart,
    SessionEnd,
    WorkspaceChanged,
    UserMsg,
    AssistantMsg,
    Delegation,
    Round,
    ToolCall,
    ToolResult,
    Steering,
    SteeringDrained,
    ConfirmationReq,
    ConfirmationRes,
    SubagentOpen,
    SubagentClose,
    SkillInvoked,
    Compaction,
    ArtifactWritten,
    FollowupQueued,
    LogTrimmed,
    WorkflowDone,
    Error,
}

impl RecordType {
    /// Every variant, in catalog order — the list a reader, a sweep or a test
    /// enumerates instead of re-deriving one.
    pub const ALL: [RecordType; 22] = [
        RecordType::SessionStart,
        RecordType::SessionEnd,
        RecordType::WorkspaceChanged,
        RecordType::UserMsg,
        RecordType::AssistantMsg,
        RecordType::Delegation,
        RecordType::Round,
        RecordType::ToolCall,
        RecordType::ToolResult,
        RecordType::Steering,
        RecordType::SteeringDrained,
        RecordType::ConfirmationReq,
        RecordType::ConfirmationRes,
        RecordType::SubagentOpen,
        RecordType::SubagentClose,
        RecordType::SkillInvoked,
        RecordType::Compaction,
        RecordType::ArtifactWritten,
        RecordType::FollowupQueued,
        RecordType::LogTrimmed,
        RecordType::WorkflowDone,
        RecordType::Error,
    ];

    pub fn as_str(&self) -> &'static str {
        match self {
            RecordType::SessionStart => "session_start",
            RecordType::SessionEnd => "session_end",
            RecordType::WorkspaceChanged => "workspace_changed",
            RecordType::UserMsg => "user_msg",
            RecordType::AssistantMsg => "assistant_msg",
            RecordType::Delegation => "delegation",
            RecordType::Round => "round",
            RecordType::ToolCall => "tool_call",
            RecordType::ToolResult => "tool_result",
            RecordType::Steering => "steering",
            RecordType::SteeringDrained => "steering_drained",
            RecordType::ConfirmationReq => "confirmation_req",
            RecordType::ConfirmationRes => "confirmation_res",
            RecordType::SubagentOpen => "subagent_open",
            RecordType::SubagentClose => "subagent_close",
            RecordType::SkillInvoked => "skill_invoked",
            RecordType::Compaction => "compaction",
            RecordType::ArtifactWritten => "artifact_written",
            RecordType::FollowupQueued => "followup_queued",
            RecordType::LogTrimmed => "log_trimmed",
            RecordType::WorkflowDone => "workflow_done",
            RecordType::Error => "error",
        }
    }

    /// Parse a catalog name. `None` for a type this build does not know —
    /// which is a readable log, not a corrupt one.
    ///
    /// Not `FromStr`: an unknown type is an expected, non-error outcome for a
    /// reader, and `Option` says that where `Result<_, Infallible-ish>` would
    /// not.
    pub fn parse(s: &str) -> Option<RecordType> {
        RecordType::ALL.into_iter().find(|t| t.as_str() == s)
    }

    /// The record kinds the writer `sync_data`s after, verbatim from §5.4's
    /// fsync policy: "`sync_data` only on `session_start`, `assistant_msg`,
    /// `workflow_done`, `confirmation_res`, `session_end`, and a 5 s timer
    /// while dirty". Everything else is flushed (one `write` syscall) and
    /// left to the timer — crash exposure is at most the current round's
    /// tail, which replay tolerates.
    pub fn is_durability_boundary(&self) -> bool {
        matches!(
            self,
            RecordType::SessionStart
                | RecordType::AssistantMsg
                | RecordType::WorkflowDone
                | RecordType::ConfirmationRes
                | RecordType::SessionEnd
        )
    }
}

/// One event, before the writer stamps it with a `seq`.
///
/// `ts` is taken when the record is *made*, not when it is written: a record
/// that waited in the channel keeps the time it describes.
#[derive(Debug, Clone)]
pub struct Record {
    pub kind: RecordType,
    pub ts: DateTime<Utc>,
    /// The run this record belongs to. Absent on main-loop/chat records
    /// (§5.4) — that absence is what distinguishes them.
    pub task_id: Option<String>,
    /// The 037 span this record belongs to (P-20) — the lead's span or a
    /// subagent's. One log, global order, subagents as a filter dimension.
    pub span_id: Option<String>,
    /// The agent instance ("research_agent::a1b2c3d4").
    pub agent: Option<String>,
    /// The payload. Always an object once written.
    pub data: Value,
}

impl Record {
    pub fn new(kind: RecordType) -> Self {
        Self {
            kind,
            ts: Utc::now(),
            task_id: None,
            span_id: None,
            agent: None,
            data: Value::Object(Map::new()),
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    pub fn task(mut self, task_id: Option<&str>) -> Self {
        self.task_id = task_id.map(str::to_string);
        self
    }

    pub fn span(mut self, span_id: Option<&str>) -> Self {
        self.span_id = span_id.map(str::to_string);
        self
    }

    pub fn agent(mut self, agent: Option<&str>) -> Self {
        self.agent = agent.map(str::to_string);
        self
    }

    /// Render the envelope as one JSON line, terminated by `\n`.
    ///
    /// `data` is assumed already capped — [`cap_data`] runs in the writer,
    /// which is also the only place that knows the assigned `seq`.
    pub(crate) fn to_line(&self, seq: u64) -> String {
        let mut env = Map::new();
        env.insert("v".into(), Value::from(ENVELOPE_VERSION));
        env.insert("seq".into(), Value::from(seq));
        env.insert(
            "ts".into(),
            Value::from(self.ts.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string()),
        );
        env.insert("type".into(), Value::from(self.kind.as_str()));
        if let Some(ref t) = self.task_id {
            env.insert("task_id".into(), Value::from(t.as_str()));
        }
        if let Some(ref s) = self.span_id {
            env.insert("span_id".into(), Value::from(s.as_str()));
        }
        if let Some(ref a) = self.agent {
            env.insert("agent".into(), Value::from(a.as_str()));
        }
        env.insert("data".into(), self.data.clone());
        // A `Map<String, Value>` cannot fail to serialise.
        let mut line = serde_json::to_string(&Value::Object(env))
            .unwrap_or_else(|_| String::from(r#"{"v":1,"type":"error","data":{}}"#));
        line.push('\n');
        line
    }
}

/// Clamp `data` to the 64 KB envelope bound.
///
/// Truncates the oversized top-level **string** fields, largest first, so the
/// record keeps its shape — the tool name, the tool_use_id and the timings
/// survive, only the payload is cut. Each cut is marked in place and the
/// record carries `_truncated.spilled_pending`, which is the marker T42's
/// `results/` spill replaces with a real stub. An object that is still over
/// the bound with every string cut (a huge *structure* rather than a huge
/// value) collapses to the stub wholesale.
///
/// Returns the capped value and whether anything was cut.
pub(crate) fn cap_data(data: Value) -> (Value, bool) {
    let original_bytes = match serde_json::to_string(&data) {
        Ok(s) if s.len() <= ENVELOPE_DATA_CAP_BYTES => return (data, false),
        Ok(s) => s.len(),
        // Unserialisable payloads cannot reach here (they are built from
        // `serde_json::json!`), but a stub is the honest answer if one does.
        Err(_) => return (stub(&Value::Null, 0), true),
    };

    let Value::Object(mut map) = data else {
        return (stub(&data, original_bytes), true);
    };

    // Largest string values first — cutting the biggest one usually suffices.
    let mut victims: Vec<(String, usize)> = map
        .iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.len())))
        .filter(|(_, len)| *len > PREVIEW_CHARS)
        .collect();
    victims.sort_by(|a, b| b.1.cmp(&a.1));

    let mut cut: Vec<String> = Vec::new();
    for (key, len) in victims {
        if serialized_len(&map) <= ENVELOPE_DATA_CAP_BYTES {
            break;
        }
        if let Some(slot) = map.get_mut(&key) {
            let preview: String = slot.as_str().unwrap_or_default().chars().take(PREVIEW_CHARS).collect();
            *slot = Value::from(format!(
                "{preview}… [truncated: {len} bytes; spill pending]"
            ));
            cut.push(key);
        }
    }

    let marker = serde_json::json!({
        "fields": cut,
        "original_bytes": original_bytes,
        "spilled_pending": true,
    });
    map.insert("_truncated".into(), marker);

    if serialized_len(&map) > ENVELOPE_DATA_CAP_BYTES {
        return (stub(&Value::Object(map), original_bytes), true);
    }
    (Value::Object(map), true)
}

fn serialized_len(map: &Map<String, Value>) -> usize {
    serde_json::to_string(map).map(|s| s.len()).unwrap_or(usize::MAX)
}

/// The whole-payload replacement: a preview plus the same
/// `spilled_pending` marker, so a structurally oversized record is still a
/// findable spill site.
fn stub(original: &Value, original_bytes: usize) -> Value {
    let rendered = serde_json::to_string(original).unwrap_or_default();
    let preview: String = rendered.chars().take(PREVIEW_CHARS).collect();
    serde_json::json!({
        "_truncated": {
            "fields": ["*"],
            "original_bytes": if original_bytes > 0 { original_bytes } else { rendered.len() },
            "spilled_pending": true,
        },
        "preview": preview,
    })
}

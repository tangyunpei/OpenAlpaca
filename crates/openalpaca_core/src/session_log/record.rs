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
    /// §5.6c — an interrupted run re-entered under its own id, naming the
    /// slice of this log its history was rebuilt from.
    Resume,
    Error,
}

impl RecordType {
    /// Every variant, in catalog order — the list a reader, a sweep or a test
    /// enumerates instead of re-deriving one.
    pub const ALL: [RecordType; 23] = [
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
        RecordType::Resume,
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
            RecordType::Resume => "resume",
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

/// The directory a session's spilled tool results live in, relative to the
/// session directory (§5.4's `results/`).
pub const RESULTS_DIR: &str = "results";

/// A tool result too large to sit inline: the bytes travel with the record and
/// the writer puts them in `results/` once, on its blocking thread (§5.4's
/// "Spill, don't truncate").
///
/// The reference is reserved by the emitter — [`SessionLogHandle::reserve_spill`](
/// crate::session_log::SessionLogHandle::reserve_spill) — because the
/// **model-visible** stub naming it has to be produced synchronously on the
/// loop's path, which cannot wait for the writer.
#[derive(Debug, Clone)]
pub struct Spill {
    /// `results/<seq>-<uid8>-<tool>.txt`, relative to the session directory.
    pub rel: String,
    /// The whole result. Written once; the record keeps only a preview.
    pub content: String,
}

/// The **model-visible** stub §5.4 specifies, verbatim.
///
/// One function so the loop, the writer's record and every test agree on the
/// wording — a model that has been taught to look for `result_ref=` must find
/// exactly this shape on every surface.
pub fn spill_stub(bytes: usize, preview: &str, rel: &str) -> String {
    format!(
        "[result too large: {bytes} bytes; first 2 KB follow]\n{preview}\n\
         [full result: result_ref=file:{rel} — use read_result to page]"
    )
}

/// The first [`PREVIEW_CHARS`] characters of a result — the same bytes the
/// record, the stub and `tool_execution_log.result_preview` all carry, so the
/// expanded-turn view renders without touching `results/` and a replay can
/// inline the preview after a spill file has been evicted (§5.4).
pub fn spill_preview(content: &str) -> String {
    content.chars().take(PREVIEW_CHARS).collect()
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
    /// Bytes for `results/` that must never sit inline (§5.4). The writer
    /// creates the file, then rewrites `data.result` into the reference plus a
    /// preview — so the payload is stored once, not twice.
    pub spill: Option<Spill>,
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
            spill: None,
        }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = data;
        self
    }

    /// Attach the bytes the writer must put in `results/` under `rel`.
    pub fn with_spill(mut self, rel: String, content: String) -> Self {
        self.spill = Some(Spill { rel, content });
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
    ///
    /// The order of the keys is [`Envelope`]'s declared order, not the key
    /// names' — see that struct for why the reader depends on it.
    pub(crate) fn to_line(&self, seq: u64) -> String {
        let env = Envelope {
            v: ENVELOPE_VERSION,
            seq,
            ts: self.ts.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            kind: self.kind.as_str(),
            task_id: self.task_id.as_deref(),
            span_id: self.span_id.as_deref(),
            agent: self.agent.as_deref(),
            data: &self.data,
        };
        // A struct of `Value`s and strings cannot fail to serialise.
        let mut line = serde_json::to_string(&env)
            .unwrap_or_else(|_| String::from(r#"{"v":1,"type":"error","data":{}}"#));
        line.push('\n');
        line
    }
}

/// The on-disk envelope, in the order it is written.
///
/// It is a struct rather than a `serde_json::Map` for one reason: `serde_json`
/// is built here **without** `preserve_order`, so a `Map` is a `BTreeMap` and
/// serialises its keys lexicographically — `agent, data, seq, …`, putting the
/// payload *before* the record's own `seq`. A derived `Serialize` keeps the
/// declared order, which is what lets the reader's cheap cursor scan find the
/// envelope's `seq` near the head of the line (`reader::scan_seq`). Absent
/// optional fields stay absent, exactly as the `Map` form left them out.
#[derive(Serialize)]
struct Envelope<'a> {
    v: u8,
    seq: u64,
    ts: String,
    #[serde(rename = "type")]
    kind: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    span_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent: Option<&'a str>,
    data: &'a Value,
}

/// The keys that carry a record's **identity** rather than its payload, at the
/// top level of `data`.
///
/// A cut never touches one of these. The reason is concrete: `tool_use_id` is
/// what pairs a `tool_call` with its `tool_result` and what the writer's index
/// row keys `log_seq` on, and the rest name the call, the run and the model. A
/// truncated record that lost them is not a smaller record, it is an anonymous
/// one.
///
/// Deeper down the rule narrows to [`is_pairing_key`]: a key called `name` or
/// `id` inside a tool's own `input` is payload the caller chose, not identity.
const IDENTITY_KEYS: &[&str] = &[
    "tool_use_id",
    "id",
    "name",
    "type",
    "kind",
    "log_seq",
    "seq",
    "task_id",
    "span_id",
    "request_id",
    "msg_id",
    "agent",
    "model",
    "ext",
];

/// How far past [`PREVIEW_CHARS`] a string must be before cutting it buys
/// more than the marker it gains.
const CUT_WORTH_IT: usize = 128;

fn is_identity(key: &str) -> bool {
    IDENTITY_KEYS.contains(&key)
}

/// The keys that **pair** records with each other below the top level, and are
/// therefore protected at depth.
///
/// Exactly two positions qualify: `tool_use_id` wherever it appears (it is the
/// join key for `tool_call`/`tool_result` and for the index row), and the
/// `id`/`name` **directly under** `tool_use[i]`, which are what make `round`'s
/// verbatim block replayable (§5.4). Everything else — including a key that
/// happens to be called `name` inside a tool's `input` — is payload, and
/// refusing to cut it collapses the record into the fallback stub for nothing.
fn is_pairing_key(path: &[Seg], key: &str) -> bool {
    if key == "tool_use_id" {
        return true;
    }
    matches!(key, "id" | "name")
        && matches!(path, [Seg::Key(container), Seg::Idx(_)] if container == "tool_use")
}

/// One step of a path to a cuttable string: `input.content`,
/// `tool_use[0].input.cmd`.
#[derive(Clone, Debug)]
enum Seg {
    Key(String),
    Idx(usize),
}

/// Clamp `data` to the 64 KB envelope bound.
///
/// Truncates the oversized strings **at any depth**, largest first, so the
/// record keeps its shape: the payload that made it big is cut, and the tool
/// name, the `tool_use_id`, the per-element ids and the timings survive.
/// `tool_call`'s `input` is an object and `round`'s `tool_use` an array, so a
/// cut that could only reach top-level strings would collapse exactly the
/// records that matter most.
///
/// Each cut is marked in place and the record carries
/// `_truncated.spilled_pending`, which is the marker T42's `results/` spill
/// replaces with a real stub. A payload no cut can shrink (a huge *structure*
/// rather than a huge value) falls back to that marker plus a preview — with
/// the identity fields still beside them, never an anonymous stub.
///
/// Returns the capped value and whether anything was cut.
pub(crate) fn cap_data(data: Value) -> (Value, bool) {
    let original_bytes = match serde_json::to_string(&data) {
        Ok(s) if s.len() <= ENVELOPE_DATA_CAP_BYTES => return (data, false),
        Ok(s) => s.len(),
        // Unserialisable payloads cannot reach here (they are built from
        // `serde_json::json!`), but a marker is the honest answer if one does.
        Err(_) => return (fallback(&Value::Null, 0, &Map::new()), true),
    };

    let Value::Object(mut map) = data else {
        return (fallback(&data, original_bytes, &Map::new()), true);
    };

    // Kept aside before anything is cut: whatever the cut cannot save, these
    // survive it.
    let identity: Map<String, Value> = map
        .iter()
        .filter(|(k, _)| is_identity(k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();

    // Every cuttable string, anywhere in the payload, largest first. A cut
    // only ever shortens a string in place, so no path is invalidated by an
    // earlier one and the list is collected once.
    let mut victims: Vec<(Vec<Seg>, usize)> = Vec::new();
    let mut path: Vec<Seg> = Vec::new();
    for (key, value) in map.iter() {
        if is_identity(key) {
            continue;
        }
        path.push(Seg::Key(key.clone()));
        collect_cuttable(value, &mut path, &mut victims);
        path.pop();
    }
    victims.sort_by(|a, b| b.1.cmp(&a.1));

    let mut cut: Vec<String> = Vec::new();
    for (path, len) in victims {
        // The marker is part of what has to fit: reserving it here keeps a
        // nearly-fitting record from being pushed back over the bound by its
        // own truncation notice.
        if serialized_len(&map) + marker_len(&cut, original_bytes) <= ENVELOPE_DATA_CAP_BYTES {
            break;
        }
        let Some(slot) = slot_mut(&mut map, &path) else {
            continue;
        };
        let preview: String = slot
            .as_str()
            .unwrap_or_default()
            .chars()
            .take(PREVIEW_CHARS)
            .collect();
        *slot = Value::from(format!("{preview}… [truncated: {len} bytes; spill pending]"));
        cut.push(render_path(&path));
    }

    map.insert("_truncated".into(), marker(&cut, original_bytes));

    if serialized_len(&map) > ENVELOPE_DATA_CAP_BYTES {
        return (fallback(&Value::Object(map), original_bytes, &identity), true);
    }
    (Value::Object(map), true)
}

/// Collect the strings worth cutting under `value`, skipping every identity
/// key on the way down.
fn collect_cuttable(value: &Value, path: &mut Vec<Seg>, out: &mut Vec<(Vec<Seg>, usize)>) {
    match value {
        Value::String(s) if s.len() > PREVIEW_CHARS + CUT_WORTH_IT => {
            out.push((path.clone(), s.len()));
        }
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                path.push(Seg::Idx(i));
                collect_cuttable(item, path, out);
                path.pop();
            }
        }
        Value::Object(fields) => {
            for (key, field) in fields {
                if is_pairing_key(path, key) {
                    continue;
                }
                path.push(Seg::Key(key.clone()));
                collect_cuttable(field, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

/// Resolve a collected path back to the slot it names.
fn slot_mut<'a>(map: &'a mut Map<String, Value>, path: &[Seg]) -> Option<&'a mut Value> {
    let (first, rest) = path.split_first()?;
    let Seg::Key(key) = first else { return None };
    let mut cursor = map.get_mut(key)?;
    for seg in rest {
        cursor = match (seg, cursor) {
            (Seg::Key(k), Value::Object(fields)) => fields.get_mut(k)?,
            (Seg::Idx(i), Value::Array(items)) => items.get_mut(*i)?,
            _ => return None,
        };
    }
    Some(cursor)
}

/// `input.content`, `tool_use[0].input.cmd` — what `_truncated.fields` names.
fn render_path(path: &[Seg]) -> String {
    let mut out = String::new();
    for seg in path {
        match seg {
            Seg::Key(k) => {
                if !out.is_empty() {
                    out.push('.');
                }
                out.push_str(k);
            }
            Seg::Idx(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

fn marker(cut: &[String], original_bytes: usize) -> Value {
    serde_json::json!({
        "fields": cut,
        "original_bytes": original_bytes,
        "spilled_pending": true,
    })
}

/// What inserting the marker will cost — its key, the separator, and room for
/// the path name the next cut will add to it.
fn marker_len(cut: &[String], original_bytes: usize) -> usize {
    serde_json::to_string(&marker(cut, original_bytes))
        .map(|s| s.len())
        .unwrap_or(0)
        + r#","_truncated":"#.len()
        + 64
}

fn serialized_len(map: &Map<String, Value>) -> usize {
    serde_json::to_string(map).map(|s| s.len()).unwrap_or(usize::MAX)
}

/// The last resort: the identity fields, the same `spilled_pending` marker,
/// and a preview of what could not be kept.
///
/// It is deliberately *not* an anonymous stub — a record that cannot say
/// which call it belongs to takes its index row and its replay key with it
/// (Critical 1).
fn fallback(original: &Value, original_bytes: usize, identity: &Map<String, Value>) -> Value {
    let rendered = serde_json::to_string(original).unwrap_or_default();
    let bytes = if original_bytes > 0 {
        original_bytes
    } else {
        rendered.len()
    };
    let mut out: Map<String, Value> = identity.clone();
    out.insert(
        "_truncated".into(),
        serde_json::json!({
            "fields": ["*"],
            "original_bytes": bytes,
            "spilled_pending": true,
        }),
    );
    let preview: String = rendered.chars().take(PREVIEW_CHARS).collect();
    out.insert("preview".into(), Value::from(preview));

    // An identity field can itself be pathological. The marker is the one
    // entry that may not be dropped to make room.
    while serialized_len(&out) > ENVELOPE_DATA_CAP_BYTES {
        let worst = out
            .iter()
            .filter(|(k, _)| k.as_str() != "_truncated")
            .max_by_key(|(_, v)| serde_json::to_string(v).map(|s| s.len()).unwrap_or(0))
            .map(|(k, _)| k.clone());
        match worst {
            Some(key) => {
                out.remove(&key);
            }
            None => break,
        }
    }
    Value::Object(out)
}
